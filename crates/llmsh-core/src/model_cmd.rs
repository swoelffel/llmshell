use std::io::BufRead;
use std::path::Path;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use anyhow::Context as _;
use regex::Regex;

// ─── Chat-only filter ───────────────────────────────────────────────────────

static INCLUDE_RE: OnceLock<Regex> = OnceLock::new();

fn include_re() -> &'static Regex {
    INCLUDE_RE.get_or_init(|| Regex::new(r"^(gpt-|o[1-9]|chatgpt-)").unwrap())
}

const EXCLUDE_PATTERNS: &[&str] = &[
    "embedding",
    "whisper",
    "tts",
    "dall-e",
    "moderation",
    "audio",
    "babbage",
    "davinci",
];

pub fn is_chat_model(id: &str) -> bool {
    if !include_re().is_match(id) {
        return false;
    }
    !EXCLUDE_PATTERNS.iter().any(|p| id.contains(p))
}

// ─── Levenshtein ────────────────────────────────────────────────────────────

pub fn levenshtein(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let m = a.len();
    let n = b.len();
    let mut dp = vec![vec![0usize; n + 1]; m + 1];
    for (i, row) in dp.iter_mut().enumerate().take(m + 1) {
        row[0] = i;
    }
    for (j, cell) in dp[0].iter_mut().enumerate().take(n + 1) {
        *cell = j;
    }
    for i in 1..=m {
        for j in 1..=n {
            dp[i][j] = if a[i - 1] == b[j - 1] {
                dp[i - 1][j - 1]
            } else {
                1 + dp[i - 1][j - 1].min(dp[i - 1][j]).min(dp[i][j - 1])
            };
        }
    }
    dp[m][n]
}

pub fn closest_match<'a>(query: &str, candidates: &'a [String]) -> Option<&'a str> {
    candidates
        .iter()
        .map(|c| (c.as_str(), levenshtein(query, c)))
        .filter(|(_, d)| *d <= 3)
        .min_by_key(|(_, d)| *d)
        .map(|(s, _)| s)
}

// ─── ModelListCache ──────────────────────────────────────────────────────────

struct CachedList {
    items: Vec<String>,
    fetched_at: Instant,
}

pub struct ModelListCache {
    inner: Mutex<Option<CachedList>>,
}

impl Default for ModelListCache {
    fn default() -> Self {
        Self {
            inner: Mutex::new(None),
        }
    }
}

impl ModelListCache {
    pub fn new() -> Self {
        Self::default()
    }

    pub async fn get_or_refresh<F, Fut>(
        &self,
        ttl: Duration,
        fetcher: F,
    ) -> anyhow::Result<Vec<String>>
    where
        F: FnOnce() -> Fut,
        Fut: std::future::Future<Output = anyhow::Result<Vec<String>>>,
    {
        {
            let guard = self.inner.lock().unwrap();
            if let Some(ref cached) = *guard {
                if cached.fetched_at.elapsed() < ttl {
                    return Ok(cached.items.clone());
                }
            }
        }
        let items = fetcher().await?;
        {
            let mut guard = self.inner.lock().unwrap();
            *guard = Some(CachedList {
                items: items.clone(),
                fetched_at: Instant::now(),
            });
        }
        Ok(items)
    }
}

// ─── REPL command implementation ─────────────────────────────────────────────

pub struct ModelCommandContext<'a> {
    pub provider: &'a dyn llmsh_llm::provider::LlmProvider,
    pub model_label: &'a std::sync::Arc<std::sync::RwLock<String>>,
    pub cache: &'a ModelListCache,
    pub config_path: Option<&'a Path>,
    pub audit: &'a std::sync::Mutex<llmsh_audit::writer::AuditWriter>,
}

pub async fn handle_model_command(
    ctx: &ModelCommandContext<'_>,
    args: &[String],
) -> anyhow::Result<()> {
    match args {
        [] => interactive_select(ctx).await,
        [a] if a == "list" => list_models(ctx).await,
        [a, id] if a == "set" => set_model_flow(ctx, id).await,
        _ => {
            println!("usage: /model | /model list | /model set <id>");
            Ok(())
        }
    }
}

async fn fetch_filtered(ctx: &ModelCommandContext<'_>) -> anyhow::Result<Vec<String>> {
    ctx.cache
        .get_or_refresh(Duration::from_secs(60), || async {
            let infos = ctx.provider.list_models().await?;
            let mut filtered: Vec<String> = infos
                .into_iter()
                .filter(|m| is_chat_model(&m.id))
                .map(|m| m.id)
                .collect();
            filtered.sort();
            Ok(filtered)
        })
        .await
}

pub async fn list_models(ctx: &ModelCommandContext<'_>) -> anyhow::Result<()> {
    let models = fetch_filtered(ctx).await?;
    if models.is_empty() {
        println!("no chat models available; check provider configuration");
        return Ok(());
    }
    let current = ctx
        .model_label
        .read()
        .map(|g| g.clone())
        .unwrap_or_else(|_| "unknown".into());
    println!("Available chat models (current: {}):", current);
    for (i, m) in models.iter().enumerate() {
        if m == &current {
            println!("  [{}] {}      <- active", i + 1, m);
        } else {
            println!("  [{}] {}", i + 1, m);
        }
    }
    Ok(())
}

pub async fn interactive_select(ctx: &ModelCommandContext<'_>) -> anyhow::Result<()> {
    interactive_select_from(ctx, &mut std::io::BufReader::new(std::io::stdin())).await
}

pub async fn interactive_select_from<R: BufRead>(
    ctx: &ModelCommandContext<'_>,
    reader: &mut R,
) -> anyhow::Result<()> {
    let models = fetch_filtered(ctx).await?;
    if models.is_empty() {
        println!("no chat models available; check provider configuration");
        return Ok(());
    }
    let current = ctx
        .model_label
        .read()
        .map(|g| g.clone())
        .unwrap_or_else(|_| "unknown".into());
    println!("Available chat models (current: {}):", current);
    for (i, m) in models.iter().enumerate() {
        if m == &current {
            println!("  [{}] {}      <- active", i + 1, m);
        } else {
            println!("  [{}] {}", i + 1, m);
        }
    }
    let n = models.len();
    loop {
        print!("Select a model [1-{}, Enter to keep, q to cancel]: ", n);
        use std::io::Write;
        let _ = std::io::stdout().flush();
        let mut line = String::new();
        reader.read_line(&mut line)?;
        let trimmed = line.trim();
        if trimmed.is_empty() {
            return Ok(());
        }
        if trimmed.eq_ignore_ascii_case("q") {
            println!("cancelled");
            return Ok(());
        }
        match trimmed.parse::<usize>() {
            Ok(idx) if idx >= 1 && idx <= n => {
                let id = models[idx - 1].clone();
                return set_model_flow(ctx, &id).await;
            }
            _ => {
                println!("invalid selection, try again");
            }
        }
    }
}

pub async fn set_model_flow(ctx: &ModelCommandContext<'_>, id: &str) -> anyhow::Result<()> {
    let models = fetch_filtered(ctx).await?;
    if !models.contains(&id.to_string()) {
        let suggestion = closest_match(id, &models);
        if let Some(s) = suggestion {
            eprintln!("unknown model: {}  (did you mean: {}?)", id, s);
        } else {
            eprintln!("unknown model: {}", id);
        }
        return Ok(());
    }

    let from = ctx
        .model_label
        .read()
        .map(|g| g.clone())
        .unwrap_or_else(|_| "unknown".into());

    ctx.provider.set_model(id).await?;

    if let Some(path) = ctx.config_path {
        let stored_id = build_stored_id(&from, id);
        crate::config::persist::set_default_model(path, &stored_id)
            .with_context(|| format!("persist model to {}", path.display()))?;
        let _ = ctx
            .audit
            .lock()
            .unwrap()
            .write(&llmsh_audit::event::AuditEvent::ModelChanged {
                ts: llmsh_audit::event::now_iso(),
                from: from.clone(),
                to: id.to_string(),
            });
        println!("model set to {} (persisted to {})", id, path.display());
    } else {
        let _ = ctx
            .audit
            .lock()
            .unwrap()
            .write(&llmsh_audit::event::AuditEvent::ModelChanged {
                ts: llmsh_audit::event::now_iso(),
                from: from.clone(),
                to: id.to_string(),
            });
        println!("model set to {}", id);
    }

    Ok(())
}

fn build_stored_id(from: &str, model_id: &str) -> String {
    if let Some((prefix, _)) = from.split_once(':') {
        format!("{}:{}", prefix, model_id)
    } else {
        model_id.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chat_model_filter_included() {
        for id in &[
            "gpt-4o-mini",
            "gpt-3.5-turbo",
            "o1-preview",
            "o3-mini",
            "chatgpt-4o-latest",
        ] {
            assert!(is_chat_model(id), "{} should be included", id);
        }
    }

    #[test]
    fn chat_model_filter_excluded() {
        for id in &[
            "text-embedding-3-small",
            "whisper-1",
            "tts-1",
            "dall-e-3",
            "gpt-4o-audio-preview",
            "babbage-002",
            "davinci-002",
        ] {
            assert!(!is_chat_model(id), "{} should be excluded", id);
        }
        assert!(!is_chat_model("omni-moderation-latest"));
    }

    #[test]
    fn levenshtein_same() {
        assert_eq!(levenshtein("abc", "abc"), 0);
    }

    #[test]
    fn levenshtein_one_sub() {
        assert_eq!(levenshtein("abc", "abd"), 1);
    }

    #[test]
    fn levenshtein_empty_to_abc() {
        assert_eq!(levenshtein("", "abc"), 3);
    }

    #[test]
    fn levenshtein_insert() {
        assert_eq!(levenshtein("kitten", "sitting"), 3);
    }

    #[test]
    fn closest_match_finds_near() {
        let candidates: Vec<String> = vec!["gpt-4o-mini".into(), "gpt-4o".into()];
        let m = closest_match("gpt-4o-mni", &candidates);
        assert_eq!(m, Some("gpt-4o-mini"));
    }

    #[test]
    fn closest_match_none_when_too_far() {
        let candidates: Vec<String> = vec!["gpt-4o-mini".into()];
        let m = closest_match("completely-different-xyz", &candidates);
        assert!(m.is_none());
    }

    #[tokio::test]
    async fn cache_returns_cached_within_ttl() {
        let cache = ModelListCache::new();
        let counter = std::sync::Arc::new(std::sync::Mutex::new(0u32));

        let c1 = counter.clone();
        let result1 = cache
            .get_or_refresh(Duration::from_secs(60), || async move {
                *c1.lock().unwrap() += 1;
                Ok(vec!["gpt-4o".to_string()])
            })
            .await
            .unwrap();

        let c2 = counter.clone();
        let result2 = cache
            .get_or_refresh(Duration::from_secs(60), || async move {
                *c2.lock().unwrap() += 1;
                Ok(vec!["gpt-4o".to_string()])
            })
            .await
            .unwrap();

        assert_eq!(*counter.lock().unwrap(), 1);
        assert_eq!(result1, result2);
    }

    #[tokio::test]
    async fn cache_refetches_after_ttl() {
        let cache = ModelListCache::new();
        let counter = std::sync::Arc::new(std::sync::Mutex::new(0u32));

        let c1 = counter.clone();
        cache
            .get_or_refresh(Duration::from_millis(1), || async move {
                *c1.lock().unwrap() += 1;
                Ok(vec!["gpt-4o".to_string()])
            })
            .await
            .unwrap();

        tokio::time::sleep(Duration::from_millis(5)).await;

        let c2 = counter.clone();
        cache
            .get_or_refresh(Duration::from_millis(1), || async move {
                *c2.lock().unwrap() += 1;
                Ok(vec!["gpt-4o".to_string()])
            })
            .await
            .unwrap();

        assert_eq!(*counter.lock().unwrap(), 2);
    }

    #[test]
    fn build_stored_id_with_prefix() {
        assert_eq!(
            build_stored_id("openai:gpt-4o-mini", "gpt-4o"),
            "openai:gpt-4o"
        );
    }

    #[test]
    fn build_stored_id_without_prefix() {
        assert_eq!(build_stored_id("gpt-4o-mini", "gpt-4o"), "gpt-4o");
    }
}

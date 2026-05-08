pub fn new_session_id() -> String {
    let now = chrono::Utc::now().format("%Y-%m-%dT%H-%M-%SZ");
    let short = uuid::Uuid::new_v4().to_string()[..8].to_string();
    format!("{}-{}", now, short)
}

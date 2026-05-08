use crate::plan::CheckedPlan;
use llmsh_policy::types::PolicyAction;

pub trait ConfirmationGate: Send + Sync {
    fn ask(&self, plan: &CheckedPlan) -> bool;
}

pub struct StdinConfirmationGate;

impl ConfirmationGate for StdinConfirmationGate {
    fn ask(&self, plan: &CheckedPlan) -> bool {
        for step in &plan.steps {
            match &step.decision.action {
                PolicyAction::Allow => continue,
                PolicyAction::Deny => return false,
                PolicyAction::RequireConfirmation { strong, phrase } => {
                    println!(
                        "Confirm action: {} (risk={:?})",
                        step.call.tool_name, step.decision.effective_risk
                    );
                    if *strong {
                        let p = phrase.clone().unwrap_or_else(|| "yes".into());
                        println!("Type exactly: {}", p);
                        let mut line = String::new();
                        if std::io::stdin().read_line(&mut line).is_err() {
                            return false;
                        }
                        if line.trim() != p {
                            return false;
                        }
                    } else {
                        print!("[y/N] ");
                        use std::io::Write;
                        let _ = std::io::stdout().flush();
                        let mut line = String::new();
                        if std::io::stdin().read_line(&mut line).is_err() {
                            return false;
                        }
                        let s = line.trim().to_lowercase();
                        if !(s == "y" || s == "yes") {
                            return false;
                        }
                    }
                }
            }
        }
        true
    }
}

pub struct AlwaysYesGate;
impl ConfirmationGate for AlwaysYesGate {
    fn ask(&self, plan: &CheckedPlan) -> bool {
        !plan.has_deny()
    }
}

pub struct AlwaysNoGate;
impl ConfirmationGate for AlwaysNoGate {
    fn ask(&self, _plan: &CheckedPlan) -> bool {
        false
    }
}

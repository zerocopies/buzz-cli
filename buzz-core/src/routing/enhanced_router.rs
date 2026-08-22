use super::model_profile::{ModelProfile, TaskType, default_profiles};
use super::metrics::{get_metrics, record_request_success, record_request_failure};
use crate::core::decision::{self, RouteProvider};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoutingDecision {
    pub model_name: String,
    pub provider: RouteProvider,
    pub reason: String,
    pub confidence: f32,
    pub estimated_latency_ms: u64,
}

/// Detect task type from prompt
pub fn classify_task(prompt: &str) -> TaskType {
    let lower = prompt.to_lowercase();
    
    // Design/wiring patterns
    if ["redesign", "ui", "interface", "font", "color", "layout", "logo", "tab"].iter().any(|k| lower.contains(*k)) {
        return TaskType::DesignWiring;
    }
    
    // Code patterns
    if ["fn ", "def ", "function", "impl", "struct", "trait", "write code", "bugfix", "compile"].iter().any(|k| lower.contains(*k)) {
        return TaskType::CodeGeneration;
    }
    
    // Reasoning patterns  
    if ["explain", "analyze", "compare", "tradeoff", "architecture", "best practice"].iter().any(|k| lower.contains(*k)) {
        return TaskType::ComplexReasoning;
    }
    
    // Quick query
    if lower.len() < 100 && prompt.split_whitespace().count() < 20 {
        return TaskType::QuickQuery;
    }
    
    TaskType::CasualChat
}

/// Select best model for a given task using scoring algorithm
pub fn select_model_with_task(prompt: &str, smart_enabled: bool) -> RoutingDecision {
    if !smart_enabled {
        // Fall back to basic routing when smart mode is off
        let base_route = crate::core::decision::decide_route(prompt, &crate::policy::RoutingConfig::default());
        return RoutingDecision {
            model_name: base_route.provider.as_str().to_string(),
            provider: base_route.provider,
            reason: base_route.reason,
            confidence: base_route.confidence,
            estimated_latency_ms: 0,
        };
    }

    let task = classify_task(prompt);
    let profiles = default_profiles();
    let complexity = decision::analyze_complexity(prompt);
    
    // Score each enabled model
    let scored_models: Vec<(f64, &ModelProfile)> = profiles
        .iter()
        .filter(|m| m.enabled)
        .map(|m| {
            let task_score = m.score_for_task(task);
            let metrics = get_metrics(&m.name);
            let success_score = metrics.success_rate();
            let latency_penalty = 1.0 / (1.0 + metrics.avg_latency_ms() / 1000.0);
            
            // Weighted scoring: task fit (40%), cost (25%), success rate (25%), latency (10%)
            let final_score = 
                (task_score * 0.4) +
                (success_score * 0.25) +
                (latency_penalty * 0.1) +
                ((1.0 - m.token_cost_per_1k * 100.0).max(0.0) * 0.25);
            
            (final_score, m)
        })
        .collect();
    
    // Pick highest-scoring model
    let (best_score, best_model) = scored_models
        .into_iter()
        .max_by(|a, b| a.0.partial_cmp(&b.0).unwrap())
        .expect("At least one model must be enabled");
    
    let reason = format!(
        "task={} complexity={} score={:.2}",
        match task {
            TaskType::DesignWiring => "design",
            TaskType::CodeGeneration => "code",
            TaskType::QuickQuery => "query",
            TaskType::ComplexReasoning => "reasoning",
            TaskType::CasualChat => "chat",
        },
        complexity,
        best_score
    );
    
    RoutingDecision {
        model_name: best_model.name.clone(),
        provider: best_model.provider,
        reason,
        confidence: (best_score * 100.0).min(100.0) as f32,
        estimated_latency_ms: (best_model.latency_s * 1000.0) as u64,
    }
}

/// Record the outcome of a request for future routing decisions
pub fn record_routing_outcome(model_name: &str, success: bool, latency_ms: u64) {
    if success {
        record_request_success(model_name, latency_ms);
    } else {
        record_request_failure(model_name);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn design_prompt_classifies_correctly() {
        let prompt = "Redesign the settings page with purple logo and SF Pro font";
        assert_eq!(classify_task(prompt), TaskType::DesignWiring);
    }
    
    #[test]
    fn code_prompt_classifies_correctly() {
        let prompt = "Fix the Rust lifetime error in this function";
        assert_eq!(classify_task(prompt), TaskType::CodeGeneration);
    }
    
    #[test]
    fn selects_model_for_task() {
        let decision = select_model_with_task("Redesign my app UI", false);
        assert!(!decision.model_name.is_empty());
        assert!(decision.confidence > 0.0);
    }
}

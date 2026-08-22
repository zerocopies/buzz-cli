use serde::{Deserialize, Serialize};
use super::metrics::get_metrics;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TaskType {
    DesignWiring,
    CodeGeneration,
    QuickQuery,
    ComplexReasoning,
    CasualChat,
}

impl std::str::FromStr for TaskType {
    type Err = ();
    
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "design" | "designwiring" => Ok(TaskType::DesignWiring),
            "code" | "coding" => Ok(TaskType::CodeGeneration),
            "query" | "quick" => Ok(TaskType::QuickQuery),
            "reasoning" | "complex" => Ok(TaskType::ComplexReasoning),
            "chat" | "casual" => Ok(TaskType::CasualChat),
            _ => Err(()),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelProfile {
    pub name: String,
    pub provider: crate::core::decision::RouteProvider,
    pub task_preference: TaskType,
    pub latency_s: f64,
    pub token_cost_per_1k: f64,
    pub enabled: bool,
}

impl ModelProfile {
    pub fn score_for_task(&self, task: TaskType) -> f64 {
        let task_score = if self.task_preference == task { 1.0 } else { 0.5 };
        let cost_score = 1.0 / (self.token_cost_per_1k + 0.01);
        let metrics = get_metrics(&self.name);
        let success_score = metrics.success_rate();
        
        // Weighted: task fit (40%), success rate (35%), cost (25%)
        (task_score * 0.4) + (success_score * 0.35) + (cost_score * 0.25)
    }
}

pub fn default_profiles() -> Vec<ModelProfile> {
    vec![
        ModelProfile {
            name: "qwen2.5-coder-1.5b".to_string(),
            provider: crate::core::decision::RouteProvider::Local,
            task_preference: TaskType::CodeGeneration,
            latency_s: 2.0,
            token_cost_per_1k: 0.0,
            enabled: true,
        },
        ModelProfile {
            name: "nvidia-nemotron-3-super".to_string(),
            provider: crate::core::decision::RouteProvider::Groq,
            task_preference: TaskType::ComplexReasoning,
            latency_s: 1.5,
            token_cost_per_1k: 0.0001,
            enabled: true,
        },
        ModelProfile {
            name: "laguna".to_string(),
            provider: crate::core::decision::RouteProvider::Groq,
            task_preference: TaskType::DesignWiring,
            latency_s: 1.8,
            token_cost_per_1k: 0.0001,
            enabled: true,
        },
    ]
}

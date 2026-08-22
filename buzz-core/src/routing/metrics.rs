use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

#[derive(Debug, Serialize, Deserialize)]
pub struct ModelMetrics {
    pub model_name: String,
    total_requests: AtomicUsize,
    successful_responses: AtomicUsize,
    total_latency_ms: AtomicU64,
    user_rating_sum: f64,
    rating_count: u32,
}

impl ModelMetrics {
    pub fn new(model_name: String) -> Self {
        Self {
            model_name,
            total_requests: AtomicUsize::new(0),
            successful_responses: AtomicUsize::new(0),
            total_latency_ms: AtomicU64::new(0),
            user_rating_sum: 0.0,
            rating_count: 0,
        }
    }
    
    pub fn success_rate(&self) -> f64 {
        let total = self.total_requests.load(Ordering::Relaxed);
        if total == 0 { 0.9 }
        else {
            self.successful_responses.load(Ordering::Relaxed) as f64 / total as f64
        }
    }
    
    pub fn avg_latency_ms(&self) -> f64 {
        let total = self.total_requests.load(Ordering::Relaxed);
        if total == 0 { 0.0 }
        else {
            self.total_latency_ms.load(Ordering::Relaxed) as f64 / total as f64
        }
    }
    
    pub fn record_success(&self, latency_ms: u64) {
        self.total_requests.fetch_add(1, Ordering::Relaxed);
        self.successful_responses.fetch_add(1, Ordering::Relaxed);
        self.total_latency_ms.fetch_add(latency_ms, Ordering::Relaxed);
    }
    
    pub fn record_failure(&self) {
        self.total_requests.fetch_add(1, Ordering::Relaxed);
    }
}

// Singleton registry for thread-safe metrics sharing
lazy_static::lazy_static! {
    pub static ref METRICS_REGISTRY: Arc<Mutex<HashMap<String, Arc<ModelMetrics>>>> = 
        Arc::new(Mutex::new(HashMap::new()));
}

pub fn get_metrics(model_name: &str) -> Arc<ModelMetrics> {
    let mut registry = METRICS_REGISTRY.lock().unwrap();
    registry
        .entry(model_name.to_string())
        .or_insert_with(|| Arc::new(ModelMetrics::new(model_name.to_string())))
        .clone()
}

pub fn record_request_success(model_name: &str, latency_ms: u64) {
    let metrics = get_metrics(model_name);
    metrics.record_success(latency_ms);
}

pub fn record_request_failure(model_name: &str) {
    let metrics = get_metrics(model_name);
    metrics.record_failure();
}

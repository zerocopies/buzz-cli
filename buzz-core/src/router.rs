use crate::core::decision::{decide_route, RouteProvider};
use crate::policy::Config;

pub struct Router {
    pub config: Config,
}

impl Router {
    pub fn new(config: Config) -> Self {
        Self { config }
    }

    pub fn route(&self, prompt: &str) -> (RouteProvider, String) {
        let r = decide_route(prompt, &self.config.routing);
        (r.provider, r.reason)
    }
}

use crate::policy::Cost as PolicyCost;

#[derive(Debug, Clone)]
pub struct Cost {
    pub total_spent: f64,
    pub budget: f64,
}

impl Cost {
    pub fn new_from_config(config: &PolicyCost) -> Self {
        Cost {
            total_spent: config.total_spent_usd,
            budget: config.daily_budget_usd,
        }
    }

    pub fn add_spent(&mut self, amount: f64) {
        self.total_spent += amount;
    }

    pub fn remaining(&self) -> f64 {
        (self.budget - self.total_spent).max(0.0)
    }

    pub fn is_over_budget(&self) -> bool {
        self.total_spent >= self.budget
    }
}

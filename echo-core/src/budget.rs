//! Token budget management — fine-grained allocation of the LLM context window.
//!
//! Divides the model's context window into four categories:
//! - System prompt
//! - Tool definitions
//! - Conversation history
//! - Output generation
//!
//! And reserves a safety margin. When the conversation history exceeds its
//! allocation, the budget signals that compression is needed.

use serde::Serialize;

/// Fine-grained token budget that divides the model's context window
/// into fixed-percentage allocations.
#[derive(Debug, Clone)]
pub struct TokenBudget {
    /// Total context window size (model-dependent, e.g. 128000 for GPT-4o).
    pub total_window: usize,
    /// Fraction of total reserved for the system prompt (default 0.10).
    pub system_prompt_pct: f64,
    /// Fraction of total reserved for tool definitions (default 0.05).
    pub tool_defs_pct: f64,
    /// Fraction of total reserved for output generation (default 0.10).
    pub output_pct: f64,
    /// Fraction of total kept as safety margin (default 0.10).
    pub safety_pct: f64,
}

impl TokenBudget {
    /// Create a default budget from the total context window size.
    ///
    /// Uses a safe allocation: 10% system, 5% tools, 10% output, 10% safety,
    /// leaving 65% for conversation history.
    pub fn new(total_window: usize) -> Self {
        Self {
            total_window,
            system_prompt_pct: 0.10,
            tool_defs_pct: 0.05,
            output_pct: 0.10,
            safety_pct: 0.10,
        }
    }

    /// Customize the allocation percentages. All values are fractions
    /// of `total_window` (e.g., `0.10` = 10%).
    ///
    /// The sum must not exceed 1.0; if it does, the conversation budget
    /// will be zero.
    pub fn with_allocations(
        mut self,
        system_pct: f64,
        tool_pct: f64,
        output_pct: f64,
        safety_pct: f64,
    ) -> Self {
        self.system_prompt_pct = system_pct;
        self.tool_defs_pct = tool_pct;
        self.output_pct = output_pct;
        self.safety_pct = safety_pct;
        self
    }

    /// Maximum tokens available for the system prompt.
    pub fn system_prompt_budget(&self) -> usize {
        (self.total_window as f64 * self.system_prompt_pct) as usize
    }

    /// Maximum tokens available for tool definitions.
    pub fn tool_definitions_budget(&self) -> usize {
        (self.total_window as f64 * self.tool_defs_pct) as usize
    }

    /// Maximum tokens reserved for LLM output.
    pub fn output_budget(&self) -> usize {
        (self.total_window as f64 * self.output_pct) as usize
    }

    /// Maximum tokens available for conversation history.
    /// This is what remains after all allocations and the safety margin.
    pub fn conversation_budget(&self) -> usize {
        let allocated =
            self.system_prompt_pct + self.tool_defs_pct + self.output_pct + self.safety_pct;
        let remaining = (1.0 - allocated).max(0.0);
        (self.total_window as f64 * remaining) as usize
    }

    /// Given estimated token counts, return a detailed [`TokenAllocation`]
    /// describing what fits and what needs trimming.
    pub fn allocate(
        &self,
        system_size: usize,
        tool_defs_size: usize,
        conversation_size: usize,
    ) -> TokenAllocation {
        let sys_ok = system_size <= self.system_prompt_budget();
        let tool_ok = tool_defs_size <= self.tool_definitions_budget();
        let conv_ok = conversation_size <= self.conversation_budget();

        let conversation_excess = if conv_ok {
            0
        } else {
            conversation_size.saturating_sub(self.conversation_budget())
        };

        let total_used = system_size + tool_defs_size + conversation_size;
        let usage_pct = if self.total_window > 0 {
            (total_used as f64 / self.total_window as f64) * 100.0
        } else {
            0.0
        };

        let output_fits = self.output_budget() > 0;

        TokenAllocation {
            system_fits: sys_ok,
            tool_defs_fit: tool_ok,
            conversation_fits: conv_ok,
            output_fits,
            conversation_excess,
            usage_pct,
        }
    }

    /// Generate a human-readable budget report for CLI display.
    pub fn report(
        &self,
        system_size: usize,
        tool_defs_size: usize,
        conversation_size: usize,
        estimated_output: usize,
    ) -> BudgetReport {
        let allocation = self.allocate(system_size, tool_defs_size, conversation_size);
        BudgetReport {
            total_window: self.total_window,
            system_prompt: system_size,
            system_prompt_budget: self.system_prompt_budget(),
            tool_definitions: tool_defs_size,
            tool_definitions_budget: self.tool_definitions_budget(),
            conversation: conversation_size,
            conversation_budget: self.conversation_budget(),
            estimated_output,
            output_budget: self.output_budget(),
            usage_pct: allocation.usage_pct,
            needs_compression: allocation.needs_compression(),
        }
    }
}

impl Default for TokenBudget {
    /// Creates a budget for a 396K model with default allocations.
    fn default() -> Self {
        Self::new(396_000)
    }
}

/// Result of checking a token budget against actual usage.
#[derive(Debug, Clone)]
pub struct TokenAllocation {
    /// Whether the system prompt fits within its budget.
    pub system_fits: bool,
    /// Whether tool definitions fit within their budget.
    pub tool_defs_fit: bool,
    /// Whether the conversation history fits within its budget.
    pub conversation_fits: bool,
    /// Whether there is room for the output budget.
    pub output_fits: bool,
    /// How many conversation tokens must be freed (0 if within budget).
    pub conversation_excess: usize,
    /// Total usage as percentage of the full window.
    pub usage_pct: f64,
}

impl TokenAllocation {
    /// True when all categories fit within their budgets.
    pub fn ok(&self) -> bool {
        self.system_fits && self.tool_defs_fit && self.conversation_fits && self.output_fits
    }

    /// True when compression should be triggered (conversation over budget).
    pub fn needs_compression(&self) -> bool {
        self.conversation_excess > 0
    }
}

/// Budget status report for CLI display.
#[derive(Debug, Clone, Serialize)]
pub struct BudgetReport {
    pub total_window: usize,
    pub system_prompt: usize,
    pub system_prompt_budget: usize,
    pub tool_definitions: usize,
    pub tool_definitions_budget: usize,
    pub conversation: usize,
    pub conversation_budget: usize,
    pub estimated_output: usize,
    pub output_budget: usize,
    pub usage_pct: f64,
    pub needs_compression: bool,
}

/// Token budget configuration that can be embedded in AgentConfig.
#[derive(Debug, Clone)]
pub struct TokenBudgetConfig {
    /// Total context window size. If `None`, auto-detected from the model.
    pub total_window: Option<usize>,
    /// Custom allocation percentages.
    pub system_pct: f64,
    pub tool_pct: f64,
    pub output_pct: f64,
    pub safety_pct: f64,
    /// Whether budget checking is enabled.
    pub enabled: bool,
}

impl Default for TokenBudgetConfig {
    fn default() -> Self {
        Self {
            total_window: None,
            system_pct: 0.10,
            tool_pct: 0.05,
            output_pct: 0.10,
            safety_pct: 0.10,
            enabled: true,
        }
    }
}

impl TokenBudgetConfig {
    /// Create a new enabled config with auto-detection.
    pub fn enabled() -> Self {
        Self::default()
    }

    /// Create a disabled config (budget checks are skipped).
    pub fn disabled() -> Self {
        Self {
            enabled: false,
            ..Default::default()
        }
    }

    /// Set an explicit context window size rather than auto-detecting.
    pub fn with_total_window(mut self, window: usize) -> Self {
        self.total_window = Some(window);
        self
    }

    /// Build a TokenBudget from this config.
    pub fn build(&self, fallback_window: usize) -> TokenBudget {
        let window = self.total_window.unwrap_or(fallback_window);
        TokenBudget::new(window).with_allocations(
            self.system_pct,
            self.tool_pct,
            self.output_pct,
            self.safety_pct,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_budget() {
        let budget = TokenBudget::new(100_000);
        assert_eq!(budget.system_prompt_budget(), 10_000); // 10%
        assert_eq!(budget.tool_definitions_budget(), 5_000); // 5%
        assert_eq!(budget.output_budget(), 10_000); // 10%
        // 100% - 10% - 5% - 10% - 10% = 65%
        assert_eq!(budget.conversation_budget(), 65_000);
    }

    #[test]
    fn default_budget_uses_396k_context_window() {
        let budget = TokenBudget::default();
        assert_eq!(budget.total_window, 396_000);
    }

    #[test]
    fn test_allocation_ok() {
        let budget = TokenBudget::new(100_000);
        let all = budget.allocate(5_000, 2_000, 30_000);
        assert!(all.ok());
        assert!(!all.needs_compression());
    }

    #[test]
    fn test_allocation_needs_compression() {
        let budget = TokenBudget::new(100_000);
        let all = budget.allocate(5_000, 2_000, 70_000); // exceeds 65K
        assert!(!all.ok());
        assert!(all.needs_compression());
        assert_eq!(all.conversation_excess, 5_000);
    }

    #[test]
    fn test_custom_allocations() {
        let budget = TokenBudget::new(100_000).with_allocations(0.05, 0.05, 0.05, 0.05);
        assert_eq!(budget.system_prompt_budget(), 5_000);
        assert_eq!(budget.conversation_budget(), 80_000); // 100% - 20% = 80%
    }

    #[test]
    fn test_disabled_config() {
        let config = TokenBudgetConfig::disabled();
        assert!(!config.enabled);
    }
}

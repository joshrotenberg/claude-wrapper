//! Test harness for auto-routing accuracy.
//!
//! Provides structured test cases, result collection, and multiple output
//! formats for validating routing prompt changes. Used by the `route_stress`
//! example and designed to feed into CI tracking (#286).
//!
//! # Usage
//!
//! ```rust,no_run
//! use claude_pool::{Pool, route_test::{RouteTestCase, RouteTestRunner}};
//! use claude_wrapper::Claude;
//!
//! # async fn example() -> anyhow::Result<()> {
//! let claude = Claude::builder().build()?;
//! let pool = Pool::builder(claude).slots(1).build().await?;
//!
//! let cases = vec![
//!     RouteTestCase::new("trivial", "What is 2+2?", &["single"]),
//! ];
//!
//! let runner = RouteTestRunner::new(&pool);
//! let summary = runner.run(&cases).await;
//! println!("{summary}");
//! # Ok(())
//! # }
//! ```

use std::time::Instant;

use serde::{Deserialize, Serialize};

use crate::auto::{AutoHint, AutoRoute};
use crate::pool::Pool;
use crate::store::PoolStore;

/// A single test case for the routing classifier.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RouteTestCase {
    /// Short label for display and tracking.
    pub label: String,
    /// The task prompt to classify.
    pub prompt: String,
    /// Acceptable route types. Use `["any"]` to accept anything.
    pub expected: Vec<String>,
    /// Optional hints to pass to the router.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hints: Option<AutoHint>,
}

impl RouteTestCase {
    /// Create a test case with one or more acceptable outcomes.
    pub fn new(label: impl Into<String>, prompt: impl Into<String>, expected: &[&str]) -> Self {
        Self {
            label: label.into(),
            prompt: prompt.into(),
            expected: expected.iter().map(|s| (*s).to_string()).collect(),
            hints: None,
        }
    }

    /// Add hints to this test case.
    pub fn with_hints(mut self, hints: AutoHint) -> Self {
        self.hints = Some(hints);
        self
    }

    /// Check if a route name matches the expected outcomes.
    pub fn matches(&self, got: &str) -> bool {
        self.expected.iter().any(|e| e == "any" || e == got)
    }
}

/// Result of running a single test case.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RouteTestResult {
    /// The test case label.
    pub label: String,
    /// The prompt that was classified.
    pub prompt: String,
    /// Expected route types.
    pub expected: Vec<String>,
    /// The route type returned, if successful.
    pub got: Option<String>,
    /// The full route decision, if successful.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub route: Option<AutoRoute>,
    /// Whether the result matched expectations.
    pub pass: bool,
    /// Error message, if the routing call failed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Wall-clock time for this case in milliseconds.
    pub elapsed_ms: u64,
    /// Number of sub-items (parallel prompts or chain steps).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sub_count: Option<usize>,
}

impl RouteTestResult {
    /// Outcome as a short string: "OK", "MISMATCH", or "ERROR".
    pub fn outcome(&self) -> &'static str {
        if self.error.is_some() {
            "ERROR"
        } else if self.pass {
            "OK"
        } else {
            "MISMATCH"
        }
    }
}

/// Summary of a full test run.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RouteTestSummary {
    /// Per-case results in execution order.
    pub results: Vec<RouteTestResult>,
    /// Total number of cases.
    pub total: usize,
    /// Cases that matched expectations.
    pub correct: usize,
    /// Cases that returned a route but didn't match.
    pub wrong: usize,
    /// Cases that errored (no route returned).
    pub errors: usize,
    /// Total wall-clock time in milliseconds.
    pub total_elapsed_ms: u64,
}

impl RouteTestSummary {
    /// Accuracy as a percentage (correct / (total - errors)).
    pub fn accuracy(&self) -> f64 {
        let denominator = self.total - self.errors;
        if denominator == 0 {
            return 0.0;
        }
        self.correct as f64 / denominator as f64 * 100.0
    }

    /// Serialize to pretty JSON.
    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).unwrap_or_default()
    }

    /// Return only the failed/errored results.
    pub fn failures(&self) -> Vec<&RouteTestResult> {
        self.results.iter().filter(|r| !r.pass).collect()
    }
}

impl std::fmt::Display for RouteTestSummary {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for r in &self.results {
            let expected = r.expected.join("|");
            write!(f, "{:<40} expected={:<10} ", r.label, expected)?;

            match r.outcome() {
                "OK" => {
                    let got = r.got.as_deref().unwrap_or("?");
                    if let Some(n) = r.sub_count {
                        write!(f, "(n={n}) ")?;
                    }
                    writeln!(f, "got={got:<10} OK  ({} ms)", r.elapsed_ms)?;
                }
                "MISMATCH" => {
                    let got = r.got.as_deref().unwrap_or("?");
                    if let Some(n) = r.sub_count {
                        write!(f, "(n={n}) ")?;
                    }
                    writeln!(f, "got={got:<10} MISMATCH  ({} ms)", r.elapsed_ms)?;
                    if let Some(route) = &r.route {
                        let json = serde_json::to_string_pretty(route).unwrap_or_default();
                        writeln!(f, "  prompt: {:?}", r.prompt)?;
                        writeln!(f, "  route:  {json}")?;
                    }
                }
                _ => {
                    let err = r.error.as_deref().unwrap_or("unknown");
                    writeln!(f, "ERROR: {err}  ({} ms)", r.elapsed_ms)?;
                    writeln!(f, "  prompt: {:?}", r.prompt)?;
                }
            }
        }

        writeln!(f)?;
        writeln!(f, "--- Results ---")?;
        writeln!(
            f,
            "Total: {}  Correct: {}  Wrong: {}  Errors: {}  Accuracy: {:.0}%  Time: {} ms",
            self.total,
            self.correct,
            self.wrong,
            self.errors,
            self.accuracy(),
            self.total_elapsed_ms,
        )?;
        Ok(())
    }
}

/// Runs routing test cases against a pool.
pub struct RouteTestRunner<'a, S: PoolStore> {
    pool: &'a Pool<S>,
}

impl<'a, S: PoolStore + 'static> RouteTestRunner<'a, S> {
    /// Create a runner for the given pool.
    pub fn new(pool: &'a Pool<S>) -> Self {
        Self { pool }
    }

    /// Run all test cases and return a summary.
    pub async fn run(&self, cases: &[RouteTestCase]) -> RouteTestSummary {
        let run_start = Instant::now();
        let mut results = Vec::with_capacity(cases.len());

        for case in cases {
            results.push(self.run_case(case).await);
        }

        let total = results.len();
        let correct = results.iter().filter(|r| r.pass).count();
        let errors = results.iter().filter(|r| r.error.is_some()).count();
        let wrong = total - correct - errors;

        RouteTestSummary {
            results,
            total,
            correct,
            wrong,
            errors,
            total_elapsed_ms: run_start.elapsed().as_millis() as u64,
        }
    }

    /// Run a single test case.
    async fn run_case(&self, case: &RouteTestCase) -> RouteTestResult {
        let start = Instant::now();

        let result = if let Some(hints) = &case.hints {
            self.pool.route_with_hints(&case.prompt, hints).await
        } else {
            self.pool.route(&case.prompt).await
        };

        let elapsed_ms = start.elapsed().as_millis() as u64;

        match result {
            Ok(route) => {
                let (got, sub_count) = match &route {
                    AutoRoute::Single { .. } => ("single".to_string(), None),
                    AutoRoute::Parallel { prompts } => {
                        ("parallel".to_string(), Some(prompts.len()))
                    }
                    AutoRoute::Chain { steps } => ("chain".to_string(), Some(steps.len())),
                };
                let pass = case.matches(&got);
                RouteTestResult {
                    label: case.label.clone(),
                    prompt: case.prompt.clone(),
                    expected: case.expected.clone(),
                    got: Some(got),
                    route: Some(route),
                    pass,
                    error: None,
                    elapsed_ms,
                    sub_count,
                }
            }
            Err(e) => RouteTestResult {
                label: case.label.clone(),
                prompt: case.prompt.clone(),
                expected: case.expected.clone(),
                got: None,
                route: None,
                pass: false,
                error: Some(e.to_string()),
                elapsed_ms,
                sub_count: None,
            },
        }
    }
}

//! Claude CLI version parsing and tested-range checks.
//!
//! [`CliVersion`] parses the `claude --version` string; the helpers
//! here compare it against the range this crate is tested against and
//! surface drift (via [`CliVersionStatus`] and a `tracing::warn!`) so a
//! host can react to an unexpectedly old or new CLI.

use std::fmt;
use std::str::FromStr;

/// A parsed Claude CLI version (semver).
///
/// # Example
///
/// ```
/// use claude_wrapper::CliVersion;
///
/// let v: CliVersion = "2.1.71".parse().unwrap();
/// assert_eq!(v.major, 2);
/// assert_eq!(v.minor, 1);
/// assert_eq!(v.patch, 71);
///
/// let min: CliVersion = "2.1.0".parse().unwrap();
/// assert!(v >= min);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub struct CliVersion {
    /// Major version component.
    pub major: u32,
    /// Minor version component.
    pub minor: u32,
    /// Patch version component.
    pub patch: u32,
}

impl CliVersion {
    /// Create a new version.
    #[must_use]
    pub fn new(major: u32, minor: u32, patch: u32) -> Self {
        Self {
            major,
            minor,
            patch,
        }
    }

    /// Parse a version from the output of `claude --version`.
    ///
    /// Expects format like `"2.1.71 (Claude Code)"` or just `"2.1.71"`.
    pub fn parse_version_output(output: &str) -> Result<Self, VersionParseError> {
        let version_str = output.split_whitespace().next().unwrap_or("");
        version_str.parse()
    }

    /// Check if this version satisfies a minimum version requirement.
    #[must_use]
    pub fn satisfies_minimum(&self, minimum: &CliVersion) -> bool {
        self >= minimum
    }

    /// Classify this version against a tested-against `[min, max]`
    /// range (both inclusive).
    ///
    /// Use to decide whether a host should warn about CLI drift.
    /// The minimum is the floor we've verified the wrapper still
    /// works against; the maximum is the upper end of the
    /// tested-against window. A version below the minimum is a hard
    /// "we know this is broken"; a version above the maximum is a
    /// soft "we haven't verified this; semantics may have drifted."
    #[must_use]
    pub fn status_within(&self, min: &CliVersion, max: &CliVersion) -> CliVersionStatus {
        if self < min {
            CliVersionStatus::OlderThanMinimum {
                found: *self,
                minimum: *min,
            }
        } else if self > max {
            CliVersionStatus::NewerUntested {
                found: *self,
                tested_max: *max,
            }
        } else {
            CliVersionStatus::Tested
        }
    }
}

/// Classification of an installed CLI version against a tested
/// range. Returned by [`CliVersion::status_within`] and
/// [`crate::Claude::cli_version_status`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum CliVersionStatus {
    /// CLI version is within the tested-against range.
    Tested,
    /// CLI is newer than the wrapper's tested-against maximum.
    /// Semantics may have drifted; the wrapper should still
    /// generally work but unexpected behavior is possible.
    NewerUntested {
        /// The installed CLI version.
        found: CliVersion,
        /// Highest CLI version the wrapper has been tested against.
        tested_max: CliVersion,
    },
    /// CLI is older than the declared minimum. The wrapper is
    /// known to behave incorrectly against this version (missing
    /// flags, different argument shapes).
    OlderThanMinimum {
        /// The installed CLI version.
        found: CliVersion,
        /// Lowest CLI version the wrapper supports.
        minimum: CliVersion,
    },
}

impl CliVersionStatus {
    /// True only for [`CliVersionStatus::Tested`]. Useful for
    /// callers branching on "should I run?" without pattern
    /// matching every variant.
    #[must_use]
    pub fn is_tested(self) -> bool {
        matches!(self, CliVersionStatus::Tested)
    }
}

impl PartialOrd for CliVersion {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for CliVersion {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.major
            .cmp(&other.major)
            .then(self.minor.cmp(&other.minor))
            .then(self.patch.cmp(&other.patch))
    }
}

/// Lowest `claude` CLI version this crate supports.
///
/// Below this the wrapper is known to behave incorrectly: flags are missing or
/// argument shapes differ. `claude agents` was repurposed in 2.1.143, which is
/// the kind of drift the floor exists to name.
pub const TESTED_CLI_VERSION_MIN: CliVersion = CliVersion {
    major: 2,
    minor: 1,
    patch: 0,
};

/// Highest `claude` CLI version this crate has been exercised against.
///
/// Above this the wrapper generally still works, but semantics may have
/// drifted, so [`Claude::cli_version_status`](crate::Claude::cli_version_status)
/// reports [`CliVersionStatus::NewerUntested`] rather than failing.
///
/// # Bumping this
///
/// Raising either bound is a claim about coverage, so it comes with work:
/// add that version to the CI matrix and fix whatever drift the contract check
/// reports. The `tested_range_matches_the_ci_contract_matrix` test below fails
/// if a bound is declared here but never exercised in CI, which is what keeps
/// the constants honest rather than aspirational.
pub const TESTED_CLI_VERSION_MAX: CliVersion = CliVersion {
    major: 2,
    minor: 1,
    patch: 999,
};

impl fmt::Display for CliVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)
    }
}

impl FromStr for CliVersion {
    type Err = VersionParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let parts: Vec<&str> = s.split('.').collect();
        if parts.len() != 3 {
            return Err(VersionParseError(s.to_string()));
        }

        let major = parts[0]
            .parse()
            .map_err(|_| VersionParseError(s.to_string()))?;
        let minor = parts[1]
            .parse()
            .map_err(|_| VersionParseError(s.to_string()))?;
        let patch = parts[2]
            .parse()
            .map_err(|_| VersionParseError(s.to_string()))?;

        Ok(Self {
            major,
            minor,
            patch,
        })
    }
}

/// Error returned when a version string cannot be parsed.
#[derive(Debug, Clone, thiserror::Error)]
#[error("invalid version string: {0:?}")]
pub struct VersionParseError(pub String);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_simple() {
        let v: CliVersion = "2.1.71".parse().unwrap();
        assert_eq!(v.major, 2);
        assert_eq!(v.minor, 1);
        assert_eq!(v.patch, 71);
    }

    #[test]
    fn test_parse_version_output() {
        let v = CliVersion::parse_version_output("2.1.71 (Claude Code)").unwrap();
        assert_eq!(v, CliVersion::new(2, 1, 71));
    }

    #[test]
    fn test_parse_version_output_trimmed() {
        let v = CliVersion::parse_version_output("  2.1.71 (Claude Code)\n").unwrap();
        assert_eq!(v, CliVersion::new(2, 1, 71));
    }

    #[test]
    fn test_display() {
        let v = CliVersion::new(2, 1, 71);
        assert_eq!(v.to_string(), "2.1.71");
    }

    #[test]
    fn test_ordering() {
        let v1 = CliVersion::new(2, 0, 0);
        let v2 = CliVersion::new(2, 1, 0);
        let v3 = CliVersion::new(2, 1, 71);
        let v4 = CliVersion::new(3, 0, 0);

        assert!(v1 < v2);
        assert!(v2 < v3);
        assert!(v3 < v4);
        assert!(v1 < v4);
    }

    #[test]
    fn test_satisfies_minimum() {
        let v = CliVersion::new(2, 1, 71);
        assert!(v.satisfies_minimum(&CliVersion::new(2, 0, 0)));
        assert!(v.satisfies_minimum(&CliVersion::new(2, 1, 71)));
        assert!(!v.satisfies_minimum(&CliVersion::new(2, 2, 0)));
        assert!(!v.satisfies_minimum(&CliVersion::new(3, 0, 0)));
    }

    #[test]
    fn test_parse_invalid() {
        assert!("not-a-version".parse::<CliVersion>().is_err());
        assert!("2.1".parse::<CliVersion>().is_err());
        assert!("2.1.x".parse::<CliVersion>().is_err());
    }

    // -- declared tested range --------------------------------------

    #[test]
    fn tested_range_is_ordered() {
        assert!(
            TESTED_CLI_VERSION_MIN <= TESTED_CLI_VERSION_MAX,
            "declared tested range is inverted: {TESTED_CLI_VERSION_MIN} > {TESTED_CLI_VERSION_MAX}"
        );
    }

    #[test]
    fn tested_range_bounds_classify_as_tested() {
        // The bounds are inclusive; a version at either end must not be
        // reported as drift, or the range would be a lie at its own edges.
        for v in [TESTED_CLI_VERSION_MIN, TESTED_CLI_VERSION_MAX] {
            assert_eq!(
                v.status_within(&TESTED_CLI_VERSION_MIN, &TESTED_CLI_VERSION_MAX),
                CliVersionStatus::Tested,
                "{v} should classify as Tested"
            );
        }
    }

    /// Extract the `claude_version` matrix axis from a workflow file, if the
    /// contract job declares one. Split out from the test below so the
    /// parsing is itself testable: the real check is vacuous until the
    /// contract job lands (#753), and a vacuous check that would not work
    /// when it stops being vacuous is worse than no check.
    fn ci_claude_version_axis(yaml: &str) -> Option<String> {
        let (_, rest) = yaml.split_once("claude_version:")?;
        Some(rest.chars().take_while(|c| *c != ']').collect())
    }

    #[test]
    fn ci_matrix_axis_parsing_works() {
        assert_eq!(ci_claude_version_axis("no matrix here"), None);
        let yaml = "    matrix:\n      claude_version: [\"2.1.0\", \"2.1.999\"]\n";
        let axis = ci_claude_version_axis(yaml).expect("axis found");
        assert!(axis.contains("2.1.0"));
        assert!(axis.contains("2.1.999"));
        assert!(!axis.contains("2.2.0"));
    }

    /// The constants claim CI coverage. This checks the claim.
    ///
    /// Returns early when the workflow file is absent (a vendored or packaged
    /// build has no `.github/`), and when no CLI-version matrix exists yet, so
    /// it starts enforcing as soon as the contract job lands (#753) rather
    /// than blocking on it. `ci_matrix_axis_parsing_works` above covers the
    /// parsing meanwhile.
    #[test]
    fn tested_range_matches_the_ci_contract_matrix() {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/.github/workflows/ci.yml");
        let Ok(yaml) = std::fs::read_to_string(path) else {
            return;
        };
        let Some(axis) = ci_claude_version_axis(&yaml) else {
            return;
        };
        for bound in [TESTED_CLI_VERSION_MIN, TESTED_CLI_VERSION_MAX] {
            assert!(
                axis.contains(&bound.to_string()),
                "declared tested bound {bound} is not in ci.yml's claude_version matrix \
                 ({axis:?}); either add it to CI or do not claim it here"
            );
        }
    }

    // -- status_within ---------------------------------------------

    #[test]
    fn status_tested_at_min() {
        let v = CliVersion::new(2, 1, 0);
        let s = v.status_within(&CliVersion::new(2, 1, 0), &CliVersion::new(2, 1, 999));
        assert_eq!(s, CliVersionStatus::Tested);
        assert!(s.is_tested());
    }

    #[test]
    fn status_tested_at_max() {
        let v = CliVersion::new(2, 1, 999);
        let s = v.status_within(&CliVersion::new(2, 1, 0), &CliVersion::new(2, 1, 999));
        assert_eq!(s, CliVersionStatus::Tested);
    }

    #[test]
    fn status_tested_in_middle() {
        let v = CliVersion::new(2, 1, 143);
        let s = v.status_within(&CliVersion::new(2, 1, 0), &CliVersion::new(2, 1, 999));
        assert_eq!(s, CliVersionStatus::Tested);
    }

    #[test]
    fn status_newer_untested_above_max() {
        let v = CliVersion::new(2, 2, 0);
        let s = v.status_within(&CliVersion::new(2, 1, 0), &CliVersion::new(2, 1, 999));
        assert_eq!(
            s,
            CliVersionStatus::NewerUntested {
                found: v,
                tested_max: CliVersion::new(2, 1, 999),
            }
        );
        assert!(!s.is_tested());
    }

    #[test]
    fn status_older_than_minimum() {
        let v = CliVersion::new(2, 0, 99);
        let s = v.status_within(&CliVersion::new(2, 1, 0), &CliVersion::new(2, 1, 999));
        assert_eq!(
            s,
            CliVersionStatus::OlderThanMinimum {
                found: v,
                minimum: CliVersion::new(2, 1, 0),
            }
        );
        assert!(!s.is_tested());
    }

    #[test]
    fn status_serializes_to_tagged_json() {
        let s = CliVersionStatus::Tested;
        assert_eq!(serde_json::to_string(&s).unwrap(), r#"{"status":"tested"}"#);

        let s = CliVersionStatus::NewerUntested {
            found: CliVersion::new(2, 2, 0),
            tested_max: CliVersion::new(2, 1, 999),
        };
        let json: serde_json::Value =
            serde_json::from_str(&serde_json::to_string(&s).unwrap()).expect("re-parse json");
        assert_eq!(json["status"], "newer_untested");
        assert_eq!(json["found"]["major"], 2);
        assert_eq!(json["tested_max"]["minor"], 1);
    }
}

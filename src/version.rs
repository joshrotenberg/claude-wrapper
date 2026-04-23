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
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CliVersion {
    pub major: u32,
    pub minor: u32,
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
}

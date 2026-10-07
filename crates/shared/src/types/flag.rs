//! [`Flag`] — the feature-flag catalog, and [`FlagSet`], one organization's
//! resolved answer.
//!
//! **A flag says whether a feature EXISTS right now.** Off removes the act,
//! never the recovery — a key stays revocable and a member stays removable.

use std::collections::BTreeMap;
use std::{fmt, str::FromStr};

use serde::{Deserialize, Serialize};

use crate::types::ParseEnumError;

/// One switchable feature. The variant list IS the catalog. Exhaustive on
/// purpose: the contract pin stops compiling when a variant is added and not
/// listed, which is worth more than a `#[non_exhaustive]` that would silence it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Flag {
    /// The connectors — Slack, Discord and the signed webhook. Off, the
    /// delivery queue PAUSES in place and drains when it is on again, and a
    /// new connection is refused; connected targets stay listed and removable.
    Connectors,
    /// Every `/v1` read lane a `telmoni_` token opens.
    PublicApi,
    /// Minting and rotating `telmoni_` tokens. The Keys page stays, because a
    /// leaked key must be revocable whatever the switch says.
    ApiTokens,
    /// Adding a member to an organization. The Members page stays readable, and
    /// an existing member keeps their access.
    Members,
    /// Making a NEW organization: provisioning one at first sign-in, and
    /// creating one on request (`POST /internal/organizations`), which off
    /// refuses. An existing one signs in regardless, and so does a person with
    /// none: they get their account alone — invitations and deletion — rather
    /// than a refusal. **Global only**: there is no organization yet to
    /// override on.
    Signup,
}

impl Flag {
    /// Every flag, in catalog order — for the resolver, the listing and the
    /// exhaustive tests.
    #[must_use]
    pub const fn all() -> [Self; 5] {
        [
            Self::Connectors,
            Self::PublicApi,
            Self::ApiTokens,
            Self::Members,
            Self::Signup,
        ]
    }

    /// A flag only the GLOBAL table can carry. A per-organization row for one
    /// would be read by nothing, so `make flag` refuses `ORG=` for it.
    #[must_use]
    pub fn is_global_only(self) -> bool {
        matches!(self, Self::Signup)
    }

    /// The wire spelling, without allocating. This is also the `key` column
    /// in both flag tables.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Connectors => "connectors",
            Self::PublicApi => "public_api",
            Self::ApiTokens => "api_tokens",
            Self::Members => "members",
            Self::Signup => "signup",
        }
    }

    /// The one sentence a refusal carries when this flag is off.
    #[must_use]
    pub fn off_detail(self) -> &'static str {
        match self {
            Self::Connectors => "Connectors are switched off right now.",
            Self::PublicApi => "The API is switched off right now.",
            Self::ApiTokens => "API keys are switched off right now.",
            Self::Members => "Adding a member is switched off right now.",
            Self::Signup => "Sign-ups are closed right now.",
        }
    }
}

impl fmt::Display for Flag {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for Flag {
    type Err = ParseEnumError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::all()
            .into_iter()
            .find(|f| f.as_str() == s)
            .ok_or_else(|| ParseEnumError::new(s, "a feature flag key"))
    }
}

/// One organization's resolved flags: every catalog entry, each with its answer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct FlagSet(BTreeMap<Flag, bool>);

impl<'de> Deserialize<'de> for FlagSet {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let raw = BTreeMap::<String, bool>::deserialize(d)?;
        Ok(Self(
            raw.into_iter()
                .filter_map(|(k, on)| k.parse::<Flag>().ok().map(|f| (f, on)))
                .collect(),
        ))
    }
}

impl FlagSet {
    /// The catalog with nothing overridden: every flag on.
    #[must_use]
    pub fn all_on() -> Self {
        Self(Flag::all().into_iter().map(|f| (f, true)).collect())
    }

    /// Whether `flag` is on. Absent is on — see the type doc.
    #[must_use]
    pub fn is_on(&self, flag: Flag) -> bool {
        self.0.get(&flag).copied().unwrap_or(true)
    }

    /// Set one flag's answer. The resolver applies global rows first, then the
    /// organization's own, so the later call wins.
    pub fn set(&mut self, flag: Flag, on: bool) {
        self.0.insert(flag, on);
    }

    /// The flags that are OFF, in catalog order — for the listing.
    pub fn off(&self) -> impl Iterator<Item = Flag> + '_ {
        Flag::all().into_iter().filter(|f| !self.is_on(*f))
    }
}

impl Default for FlagSet {
    fn default() -> Self {
        Self::all_on()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wire_spelling_is_snake_case_and_is_the_key_column() {
        for f in Flag::all() {
            assert_eq!(
                serde_json::to_string(&f).unwrap(),
                format!("\"{}\"", f.as_str())
            );
        }
    }

    #[test]
    fn from_str_round_trips_and_rejects_near_misses() {
        for f in Flag::all() {
            assert_eq!(f.to_string().parse::<Flag>().unwrap(), f);
        }
        assert!("Logs".parse::<Flag>().is_err());
        assert!("api-tokens".parse::<Flag>().is_err());
        assert!("".parse::<Flag>().is_err());
    }

    #[test]
    fn all_lists_every_variant_exactly_once() {
        let all = Flag::all();
        let mut keys: Vec<&str> = all.iter().map(|f| f.as_str()).collect();
        keys.sort_unstable();
        keys.dedup();
        assert_eq!(
            keys.len(),
            all.len(),
            "a duplicated key would shadow a flag"
        );
    }

    #[test]
    fn a_resolved_set_serializes_as_an_object_keyed_by_wire_spelling() {
        let mut set = FlagSet::all_on();
        set.set(Flag::ApiTokens, false);
        let json = serde_json::to_value(&set).unwrap();
        assert_eq!(json["api_tokens"], false);
        assert_eq!(json["members"], true);
        assert_eq!(json.as_object().unwrap().len(), Flag::all().len());
        let back: FlagSet = serde_json::from_value(json).unwrap();
        assert_eq!(back, set);
    }

    #[test]
    fn an_absent_key_reads_as_on_because_absence_is_not_a_decision() {
        let set: FlagSet = serde_json::from_str(r#"{"api_tokens": false}"#).unwrap();
        assert!(!set.is_on(Flag::ApiTokens));
        assert!(set.is_on(Flag::Members));
        assert_eq!(set.off().collect::<Vec<_>>(), vec![Flag::ApiTokens]);
    }

    #[test]
    fn a_key_this_binary_does_not_know_is_skipped_not_fatal() {
        let set: FlagSet =
            serde_json::from_str(r#"{"api_tokens": false, "spans": false, "x": true}"#).unwrap();
        assert!(!set.is_on(Flag::ApiTokens));
        assert_eq!(set.off().collect::<Vec<_>>(), vec![Flag::ApiTokens]);
        assert_eq!(set.0.len(), 1);
    }

    #[test]
    fn every_off_sentence_reads_as_customer_text() {
        for f in Flag::all() {
            let s = f.off_detail();
            assert!(s.ends_with('.'), "{f}: a sentence ends with a period");
            assert!(
                s.chars().next().is_some_and(char::is_uppercase),
                "{f}: a sentence starts with a capital"
            );
            assert!(!s.contains(f.as_str()), "{f}: the key is not customer text");
            assert!(!s.contains('_'), "{f}: no identifier in a toast");
        }
    }

    /// Adding a flag is a claim that something reads it. A flag no production
    /// code reads is a switch wired to nothing, which is worse than no switch —
    /// it reads as control. A test naming a flag does not count as a reader.
    #[test]
    fn every_flag_has_a_reader() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let mut files = Vec::new();
        for dir in ["crates", "web/app", "web/lib", "web/components"] {
            collect_sources(&root.join(dir), &mut files);
        }
        assert!(
            files.len() > 100,
            "scanned only {} files — is the repo root wrong?",
            files.len()
        );
        let this_file = std::path::Path::new(file!())
            .file_name()
            .unwrap()
            .to_owned();
        let haystack: String = files
            .iter()
            .filter(|p| p.file_name() != Some(this_file.as_os_str()))
            .filter_map(|p| std::fs::read_to_string(p).ok())
            .flat_map(|src| {
                src.lines()
                    .filter(|l| {
                        let t = l.trim_start();
                        !(t.starts_with("//") || t.starts_with("/*") || t.starts_with('*'))
                    })
                    .map(|l| format!("{l}\n"))
                    .collect::<Vec<_>>()
            })
            .collect();
        for f in Flag::all() {
            let rust = format!("Flag::{f:?}");
            let ts = format!("Flag.{f:?}");
            assert!(
                haystack.contains(&rust) || haystack.contains(&ts),
                "{f}: no reader anywhere in crates/ or web/ — a flag nothing reads is a lie"
            );
        }
    }

    fn collect_sources(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let name = entry.file_name();
            if name == "node_modules" || name == "target" || name == ".next" || name == "tests" {
                continue;
            }
            let file = name.to_string_lossy();
            if file.ends_with(".test.ts") || file.ends_with(".test.tsx") {
                continue;
            }
            if path.is_dir() {
                collect_sources(&path, out);
            } else if matches!(
                path.extension().and_then(|e| e.to_str()),
                Some("rs" | "ts" | "tsx")
            ) {
                out.push(path);
            }
        }
    }
}

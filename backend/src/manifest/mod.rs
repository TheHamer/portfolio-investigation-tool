//! The dataset manifest: valuation calendar, `as_of` date and expected counts.

mod error;

pub use error::ManifestError;

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use chrono::NaiveDate;
use serde::Deserialize;

use crate::domain::{Currency, parse_date};

#[derive(Debug, Deserialize)]
struct RawManifest {
    dataset_id: String,
    as_of: String,
    base_currency: String,
    valuation_dates: Vec<String>,
    #[serde(default)]
    file_counts: BTreeMap<String, usize>,
}

/// The validated manifest. Fields are private and the only constructor is
/// [`Manifest::load`], so every instance has passed validation; callers read
/// it through the accessors below.
#[derive(Debug, Clone)]
pub struct Manifest {
    dataset_id: String,
    as_of: NaiveDate,
    valuation_dates: BTreeSet<NaiveDate>,
    file_counts: BTreeMap<String, usize>,
}

impl Manifest {
    /// Reads, parses and validates the manifest at `path`.
    pub fn load(path: &Path) -> Result<Self, ManifestError> {
        let text = std::fs::read_to_string(path).map_err(|source| ManifestError::Read {
            path: path.to_path_buf(),
            source,
        })?;
        let raw: RawManifest = serde_json::from_str(&text)?;
        Self::from_raw(raw)
    }

    /// The single place a `Manifest` is created: convert the raw strings to
    /// typed values, then validate before handing it out. The base currency
    /// is checked here because the manifest does not keep it.
    fn from_raw(raw: RawManifest) -> Result<Self, ManifestError> {
        // GBP is the only supported reporting currency.
        if !matches!(raw.base_currency.parse(), Ok(Currency::Gbp)) {
            return Err(ManifestError::UnsupportedBaseCurrency(raw.base_currency));
        }

        let as_of = parse_date(&raw.as_of, "as_of")?;
        let valuation_dates = raw
            .valuation_dates
            .iter()
            .map(|d| parse_date(d, "valuation_dates"))
            .collect::<Result<BTreeSet<_>, _>>()?;

        let manifest = Self {
            dataset_id: raw.dataset_id,
            as_of,
            valuation_dates,
            file_counts: raw.file_counts,
        };

        manifest.validate()?;
        Ok(manifest)
    }

    /// Invariants on the typed values: the summary date must be the last day
    /// of the valuation calendar.
    fn validate(&self) -> Result<(), ManifestError> {
        if self.valuation_dates.last() != Some(&self.as_of) {
            return Err(ManifestError::AsOfNotFinal(self.as_of));
        }

        Ok(())
    }

    pub fn dataset_id(&self) -> &str {
        &self.dataset_id
    }

    /// Final valuation date; every summary is calculated as of this date.
    pub fn as_of(&self) -> NaiveDate {
        self.as_of
    }

    /// The only dates on which trades may be executed, in order.
    pub fn valuation_dates(&self) -> &BTreeSet<NaiveDate> {
        &self.valuation_dates
    }

    /// Expected data-row count for a file, keyed by its path relative to the
    /// dataset root (for example `reference/prices.csv`).
    pub fn expected_file_count(&self, path: &str) -> Option<usize> {
        self.file_counts.get(path).copied()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn raw(as_of: &str, currency: &str, valuation_dates: &[&str]) -> RawManifest {
        RawManifest {
            dataset_id: "x".to_owned(),
            as_of: as_of.to_owned(),
            base_currency: currency.to_owned(),
            valuation_dates: valuation_dates.iter().map(|d| (*d).to_owned()).collect(),
            file_counts: BTreeMap::new(),
        }
    }

    fn date(y: i32, m: u32, d: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, d).unwrap()
    }

    #[test]
    fn accessors_expose_the_validated_values() {
        let mut raw = raw("2026-01-06", "GBP", &["2026-01-05", "2026-01-06"]);
        raw.dataset_id = "ds-test".to_owned();
        raw.file_counts
            .insert("reference/prices.csv".to_owned(), 1436);
        let manifest = Manifest::from_raw(raw).unwrap();

        assert_eq!(manifest.dataset_id(), "ds-test");
        assert_eq!(manifest.as_of(), date(2026, 1, 6));
        assert_eq!(
            manifest
                .valuation_dates()
                .iter()
                .copied()
                .collect::<Vec<_>>(),
            vec![date(2026, 1, 5), date(2026, 1, 6)]
        );
        assert_eq!(
            manifest.expected_file_count("reference/prices.csv"),
            Some(1436)
        );
        assert_eq!(manifest.expected_file_count("prices.csv"), None);
    }

    #[test]
    fn only_gbp_is_accepted_as_base_currency() {
        for currency in ["USD", "EUR", "gbp", ""] {
            assert!(
                matches!(
                    Manifest::from_raw(raw("2026-01-05", currency, &["2026-01-05"])),
                    Err(ManifestError::UnsupportedBaseCurrency(c)) if c == currency
                ),
                "{currency:?}"
            );
        }

        let error = Manifest::from_raw(raw("2026-01-05", "USD", &["2026-01-05"])).unwrap_err();
        assert_eq!(
            error.to_string(),
            "manifest base currency must be GBP, found USD"
        );
    }

    #[test]
    fn supplied_manifest_loads() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../data/manifest.json");
        let manifest = Manifest::load(&path).unwrap();

        assert_eq!(manifest.dataset_id(), "ds-f77160d14d37a72b");
        assert_eq!(manifest.as_of(), date(2026, 3, 27));
        assert_eq!(manifest.valuation_dates().len(), 60);
        assert_eq!(
            manifest.expected_file_count("uploads/trades_initial.csv"),
            Some(600)
        );
    }

    #[test]
    fn as_of_must_be_final_valuation_date() {
        let days = ["2026-01-05", "2026-01-06"];
        let manifest = Manifest::from_raw(raw("2026-01-06", "GBP", &days)).unwrap();
        assert_eq!(manifest.as_of(), date(2026, 1, 6));

        assert!(matches!(
            Manifest::from_raw(raw("2026-01-05", "GBP", &days)),
            Err(ManifestError::AsOfNotFinal(_))
        ));
    }

    #[test]
    fn each_manifest_rule_has_its_own_error() {
        assert!(matches!(
            Manifest::from_raw(raw("2026-01-05", "USD", &["2026-01-05"])),
            Err(ManifestError::UnsupportedBaseCurrency(c)) if c == "USD"
        ));
        assert!(matches!(
            Manifest::from_raw(raw("05/01/2026", "GBP", &["05/01/2026"])),
            Err(ManifestError::InvalidDate(_))
        ));

        let dir = TempDir::new().unwrap();
        let path = dir.path().join("manifest.json");
        std::fs::write(&path, "{not json").unwrap();

        assert!(matches!(Manifest::load(&path), Err(ManifestError::Json(_))));

        let error = Manifest::load(Path::new("/no/such/manifest.json")).unwrap_err();
        assert!(matches!(error, ManifestError::Read { .. }));
        assert!(
            error.to_string().contains("/no/such/manifest.json"),
            "{error}"
        );
    }
}

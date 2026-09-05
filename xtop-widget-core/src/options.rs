//! Parse helpers for per-widget layout `options` objects (DR-UX1).
//!
//! Shared by the widget crates and the `xtop-widget-blocks` pack.
//!
//! Layout nodes may carry an `options` JSON object (kept verbatim by the
//! `xtop-layout` model); the kernel exposes the object of the widget being
//! rendered through [`xtop_widget_api::WidgetState::widget_options`]. This
//! module turns those JSON values into the documented choices the base pack
//! renderers switch on.
//!
//! **The recognized keys, defaults and examples live in `docs/widgets.md`
//! (section "Layout options per widget") — that file is the canonical
//! reference.** Rule of thumb: unknown keys and malformed values are ignored
//! (the renderer keeps its default), because `None`/absent options must
//! always reproduce the pre-options rendering byte for byte (DR-UX2).
//!
//! The `xtop-widget-blocks` pack imports this module (it is a workspace
//! member of the same repository); see `packs/xtop-widget-blocks/src/lib.rs`.

use serde_json::Value;

/// The value of a string-valued option key.
///
/// `None` when the key is absent, not a string, or the object itself is
/// missing. Renderers fall back to their documented default.
pub fn string<'a>(options: &'a Value, key: &str) -> Option<&'a str> {
    options.get(key).and_then(Value::as_str)
}

/// The value of a bool-valued option key.
///
/// Returns `None` when the key is absent or not a boolean.
pub fn boolean(options: &Value, key: &str) -> Option<bool> {
    options.get(key).and_then(Value::as_bool)
}

/// A list of names (e.g. interface or mount names) under `key`.
///
/// `None` when the key is absent or not an array of strings. Malformed
/// entries are skipped; an array with no usable entry yields `Some(vec![])`
/// (the caller decides the fallback, usually "all").
pub fn name_list(options: &Value, key: &str) -> Option<Vec<String>> {
    let entries = options.get(key)?.as_array()?;
    let names: Vec<String> = entries
        .iter()
        .filter_map(Value::as_str)
        .map(str::to_owned)
        .collect();
    Some(names)
}

/// A string list under `key` (an array of names) or the special value
/// `"all"`.
///
/// Returns `Some(None)` for `"all"`/absent and `Some(Some(names))` for a
/// usable array; `None` only when the value is a string that is not `"all"`
/// or a non-array non-string value.
pub fn all_or_names(options: &Value, key: &str) -> Option<Option<Vec<String>>> {
    match options.get(key) {
        None => Some(None),
        Some(value) if value.as_str() == Some("all") => Some(None),
        Some(value) if value.is_array() => name_list(options, key).map(Some),
        Some(_) => None,
    }
}

/// Parse a CPU-core subset spec: `"all"` or a comma list of ids and inclusive
/// ranges, e.g. `"0,2,4-7"` (machine-share).
///
/// `"all"`/absent keys yield [`CoreSelection::All`]; a valid spec yields
/// [`CoreSelection::Ids`] with ascending, deduplicated ids. Malformed specs
/// also yield `All` — renderers must never break on a bad value.
pub fn core_selection(options: &Value, key: &str) -> CoreSelection {
    match string(options, key) {
        None | Some("all") => CoreSelection::All,
        Some(spec) => match parse_core_spec(spec) {
            Some(ids) => CoreSelection::Ids(ids),
            None => CoreSelection::All,
        },
    }
}

/// How the cpu widget restricts the logical cores it shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CoreSelection {
    /// Every core the snapshot carries (the default).
    All,
    /// A concrete set of `cpu_id` numbers from a `"0,2,4-7"` spec.
    Ids(Vec<usize>),
}

impl CoreSelection {
    /// Resolve the selection against the snapshot's cores: matching ids are
    /// kept (sorted ascending by `cpu_id`); an empty or unmatched selection
    /// falls back to every core so the widget never goes blank.
    pub fn resolve<'a>(
        &self,
        cpus: &'a [xtop_plugin_api::model::CpuInfo],
    ) -> Vec<&'a xtop_plugin_api::model::CpuInfo> {
        match self {
            CoreSelection::All => cpus.iter().collect(),
            CoreSelection::Ids(ids) => {
                let selected: Vec<&xtop_plugin_api::model::CpuInfo> = ids
                    .iter()
                    .filter_map(|id| cpus.iter().find(|cpu| cpu.cpu_id == *id))
                    .collect();
                if selected.is_empty() {
                    cpus.iter().collect()
                } else {
                    selected
                }
            }
        }
    }
}

/// Parse `"0,2,4-7"` into `[0, 2, 4, 5, 6, 7]`.
///
/// Rules: comma-separated items; an item is either a plain id or an inclusive
/// `a-b` range (`a <= b`); ids are non-negative. Any malformed item makes the
/// whole spec invalid (`None`). The result is sorted and deduplicated.
pub fn parse_core_spec(spec: &str) -> Option<Vec<usize>> {
    let mut ids: Vec<usize> = Vec::new();
    for item in spec.split(',') {
        let item = item.trim();
        if item.is_empty() {
            return None;
        }
        if let Some((a, b)) = item.split_once('-') {
            let start: usize = a.trim().parse().ok()?;
            let end: usize = b.trim().parse().ok()?;
            if start > end {
                return None;
            }
            ids.extend(start..=end);
        } else {
            ids.push(item.parse().ok()?);
        }
    }
    ids.sort_unstable();
    ids.dedup();
    Some(ids)
}

/// A parsed chart-style choice for the cpu widget history area.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CpuChart {
    /// One machine-wide average line over every core history (the default).
    Average,
    /// One history line per shown core, colored from the series ramp.
    PerCore,
}

impl CpuChart {
    /// `"per-core"` selects the per-core view; any other/missing value keeps
    /// the average view (unknown values fall back, never break rendering).
    pub fn from_options(options: Option<&Value>, key: &str) -> Self {
        match options.and_then(|o| string(o, key)) {
            Some("per-core") => CpuChart::PerCore,
            _ => CpuChart::Average,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn string_and_boolean_ignore_wrong_types() {
        let opts = json!({ "s": "x", "b": true, "n": 5 });
        assert_eq!(string(&opts, "s"), Some("x"));
        assert_eq!(string(&opts, "b"), None);
        assert_eq!(string(&opts, "missing"), None);
        assert_eq!(boolean(&opts, "b"), Some(true));
        assert_eq!(boolean(&opts, "n"), None);
        assert_eq!(boolean(&opts, "missing"), None);
    }

    #[test]
    fn name_list_only_accepts_arrays_of_strings() {
        assert_eq!(
            name_list(&json!({ "l": ["a", "b"] }), "l"),
            Some(vec!["a".to_owned(), "b".to_owned()])
        );
        assert_eq!(
            name_list(&json!({ "l": ["a", 5] }), "l"),
            Some(vec!["a".to_owned()])
        );
        assert_eq!(name_list(&json!({ "l": "all" }), "l"), None);
        assert_eq!(name_list(&json!({}), "l"), None);
    }

    #[test]
    fn core_spec_parses_ranges_and_dedups() {
        assert_eq!(parse_core_spec("0,2,4-7"), Some(vec![0, 2, 4, 5, 6, 7]));
        assert_eq!(parse_core_spec("7-7"), Some(vec![7]));
        assert_eq!(parse_core_spec("4,4,4"), Some(vec![4]));
        assert_eq!(parse_core_spec("7-4"), None);
        assert_eq!(parse_core_spec("0,x"), None);
        assert_eq!(parse_core_spec("0,"), None);
        assert_eq!(parse_core_spec(""), None);
        assert_eq!(parse_core_spec("all"), None);
    }

    #[test]
    fn core_selection_maps_all_and_specs() {
        assert_eq!(core_selection(&json!({}), "cores"), CoreSelection::All);
        assert_eq!(
            core_selection(&json!({ "cores": "all" }), "cores"),
            CoreSelection::All
        );
        assert_eq!(
            core_selection(&json!({ "cores": "0,2-3" }), "cores"),
            CoreSelection::Ids(vec![0, 2, 3])
        );
        // Malformed spec falls back to "all" (never break rendering).
        assert_eq!(
            core_selection(&json!({ "cores": "nope" }), "cores"),
            CoreSelection::All
        );
    }

    #[test]
    fn cpu_chart_falls_back_to_average() {
        assert_eq!(CpuChart::from_options(None, "chart"), CpuChart::Average);
        assert_eq!(
            CpuChart::from_options(Some(&json!({})), "chart"),
            CpuChart::Average
        );
        assert_eq!(
            CpuChart::from_options(Some(&json!({ "chart": "average" })), "chart"),
            CpuChart::Average
        );
        assert_eq!(
            CpuChart::from_options(Some(&json!({ "chart": "per-core" })), "chart"),
            CpuChart::PerCore
        );
        assert_eq!(
            CpuChart::from_options(Some(&json!({ "chart": 42 })), "chart"),
            CpuChart::Average
        );
    }
}

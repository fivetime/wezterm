//! Find, as a dialog drives it (SecureCRT's "Find": what to find, whole
//! words only, the case, the direction, around the ends or not; NativeTerm
//! shows one): each call finds the next match from the one shown, or from
//! what the pane shows, selects it and brings it into view. No mode is
//! entered and no keys are taken, as the search overlay does: the pane
//! stays as it is, the match is its selection.

use super::TermWindow;
use crate::selection::{SelectionCoordinate, SelectionRange, SelectionX};
use luahelper::impl_lua_conversion_dynamic;
use mux::pane::{PaneId, Pattern, SearchResult};
use mux::Mux;
use std::ops::Range;
use wezterm_dynamic::{FromDynamic, ToDynamic};
use wezterm_term::StableRowIndex;
use window::WindowOps;

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, PartialEq, FromDynamic, ToDynamic)]
pub struct FindArgs {
    pub text: String,
    #[dynamic(default)]
    pub match_case: bool,
    #[dynamic(default)]
    pub whole_word: bool,
    /// From the last match on to the first, and back.
    #[dynamic(default = "default_true")]
    pub wrap: bool,
    /// Towards the scrollback's start.
    #[dynamic(default = "default_true")]
    pub up: bool,
}
impl_lua_conversion_dynamic!(FindArgs);

#[derive(Debug, Clone, Default, PartialEq, FromDynamic, ToDynamic)]
pub struct FindResult {
    /// How many matches the pane has.
    pub count: usize,
    /// The one shown, counted from the scrollback's start (1 the first);
    /// 0: none, there is none or no more that way.
    pub position: usize,
}
impl_lua_conversion_dynamic!(FindResult);

/// `text` for a regular expression to find it as it is.
fn literal(text: &str) -> String {
    let mut escaped = String::with_capacity(text.len() + 8);
    for c in text.chars() {
        if r"\.+*?()|[]{}^$#&-~".contains(c) {
            escaped.push('\\');
        }
        escaped.push(c);
    }
    escaped
}

/// What the pane is searched for.
pub fn pattern(args: &FindArgs) -> Pattern {
    if args.whole_word {
        let case = if args.match_case { "" } else { "(?i)" };
        Pattern::Regex(format!(r"{case}\b{}\b", literal(&args.text)))
    } else if args.match_case {
        Pattern::CaseSensitiveString(args.text.clone())
    } else {
        Pattern::CaseInSensitiveString(args.text.clone())
    }
}

/// The match to show next, of `results` in the pane's order: the one
/// before or after `current` (the match shown now), or without one the
/// nearest that way from what the pane shows (`viewport`: the last that
/// begins above its end, the first that begins at its start or below).
/// Past the last one that way: the first from the other end when `wrap`.
pub fn choose(
    results: &[SearchResult],
    current: Option<usize>,
    viewport: Range<StableRowIndex>,
    up: bool,
    wrap: bool,
) -> Option<usize> {
    if results.is_empty() {
        return None;
    }
    let last = results.len() - 1;
    let next = match (current, up) {
        (Some(idx), true) => idx.checked_sub(1),
        (Some(idx), false) => Some(idx + 1).filter(|&n| n <= last),
        (None, true) => results.iter().rposition(|r| r.start_y < viewport.end),
        (None, false) => results.iter().position(|r| r.start_y >= viewport.start),
    };
    match (next, wrap, up) {
        (Some(idx), ..) => Some(idx),
        (None, true, true) => Some(last),
        (None, true, false) => Some(0),
        (None, false, _) => None,
    }
}

fn selection_of(result: &SearchResult) -> SelectionRange {
    SelectionRange {
        start: SelectionCoordinate::x_y(result.start_x, result.start_y),
        end: SelectionCoordinate::x_y(result.end_x.saturating_sub(1), result.end_y),
    }
}

impl TermWindow {
    /// The next of `results` (the pane's matches, in its order) is shown:
    /// selected, and scrolled to where it is not in view.
    pub fn show_found(
        &mut self,
        pane_id: PaneId,
        results: &[SearchResult],
        args: &FindArgs,
    ) -> FindResult {
        let Some(pane) = Mux::try_get().and_then(|mux| mux.get_pane(pane_id)) else {
            return FindResult::default();
        };
        let dims = pane.get_dimensions();
        let rows = dims.viewport_rows as StableRowIndex;
        let top = self.get_viewport(pane_id).unwrap_or(dims.physical_top);
        // the match shown now: the selection, when it is one of them
        let selected = self
            .selection(pane_id)
            .range
            .as_ref()
            .map(|range| range.normalize());
        let current = selected.and_then(|selected| {
            results.iter().position(|r| {
                let found = selection_of(r);
                found.start == selected.start
                    && found.end.y == selected.end.y
                    && match (found.end.x, selected.end.x) {
                        (SelectionX::Cell(a), SelectionX::Cell(b)) => a == b,
                        _ => false,
                    }
            })
        });
        let Some(idx) = choose(results, current, top..top + rows, args.up, args.wrap) else {
            return FindResult {
                count: results.len(),
                position: 0,
            };
        };
        let found = &results[idx];
        {
            let mut selection = self.selection(pane_id);
            selection.seqno = pane.get_current_seqno();
            selection.rectangular = false;
            selection.origin = Some(selection_of(found).start);
            selection.range = Some(selection_of(found));
        }
        if found.start_y < top || found.end_y >= top + rows {
            // in the middle of what the pane shows
            self.set_viewport(pane_id, Some(found.start_y - rows / 2), dims);
        }
        if let Some(window) = self.window.as_ref() {
            window.invalidate();
        }
        FindResult {
            count: results.len(),
            position: idx + 1,
        }
    }
}

#[cfg(test)]
mod test {
    use super::*;

    fn args(text: &str, match_case: bool, whole_word: bool) -> FindArgs {
        FindArgs {
            text: text.to_string(),
            match_case,
            whole_word,
            wrap: true,
            up: true,
        }
    }

    #[test]
    fn what_is_searched_for() {
        assert_eq!(
            pattern(&args("Err", false, false)),
            Pattern::CaseInSensitiveString("Err".to_string())
        );
        assert_eq!(
            pattern(&args("Err", true, false)),
            Pattern::CaseSensitiveString("Err".to_string())
        );
        assert_eq!(
            pattern(&args("eth0", true, true)),
            Pattern::Regex(r"\beth0\b".to_string())
        );
        // what a regular expression would read as its own is the text's
        assert_eq!(
            pattern(&args("10.32.16.2 (a|b)", false, true)),
            Pattern::Regex(r"(?i)\b10\.32\.16\.2 \(a\|b\)\b".to_string())
        );
        let whole = regex::Regex::new(&pattern(&args("eth0", false, true))).unwrap();
        assert!(whole.is_match("up ETH0: ok") && !whole.is_match("veth01"));
    }

    fn at(row: StableRowIndex) -> SearchResult {
        SearchResult {
            start_y: row,
            start_x: 0,
            end_y: row,
            end_x: 3,
            match_id: 0,
        }
    }

    #[test]
    fn the_next_match() {
        let results = [at(2), at(40), at(41), at(90)];
        let shown = 30..54;
        // the first find: from what the pane shows
        assert_eq!(choose(&results, None, shown.clone(), true, true), Some(2));
        assert_eq!(choose(&results, None, shown.clone(), false, true), Some(1));
        // then from the match shown
        assert_eq!(
            choose(&results, Some(2), shown.clone(), true, true),
            Some(1)
        );
        assert_eq!(
            choose(&results, Some(2), shown.clone(), false, true),
            Some(3)
        );
        // past the ends: around, or no more
        assert_eq!(
            choose(&results, Some(0), shown.clone(), true, true),
            Some(3)
        );
        assert_eq!(choose(&results, Some(0), shown.clone(), true, false), None);
        assert_eq!(
            choose(&results, Some(3), shown.clone(), false, true),
            Some(0)
        );
        assert_eq!(choose(&results, Some(3), shown.clone(), false, false), None);
        // nothing that way from what is shown
        assert_eq!(choose(&results, None, 0..2, true, false), None);
        assert_eq!(choose(&results, None, 0..2, true, true), Some(3));
        assert_eq!(choose(&results, None, 91..115, false, false), None);
        assert_eq!(choose(&results, None, 91..115, false, true), Some(0));
        assert_eq!(choose(&[], None, shown, true, true), None);
    }
}

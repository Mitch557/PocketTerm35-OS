//! Dynamic workspaces, `[apps] workspaces = "dynamic"`, and the way back to
//! fixed ones.
//!
//! Tiled windows sit on workspaces 1..n in the order they opened, with no gaps.
//! A new one goes on the end. When one closes, the rest shift down. The window
//! switcher moves a window one place left or right. Home, the launcher, is the
//! empty workspace past the end.

use pt35_common::sway::Window;

/// What it takes to make sway match the order.
#[derive(Debug, Default, PartialEq)]
pub struct Plan {
    /// Tiled windows, first to last. Window `i` belongs on workspace `i + 1`.
    pub order: Vec<i64>,
    /// Container moves, `(id, workspace)`, sent in this order.
    pub moves: Vec<(i64, u8)>,
    /// Tiled windows seen for the first time, in the order they were added.
    pub added: Vec<i64>,
}

/// Where focus goes once the moves are made.
#[derive(Debug, PartialEq)]
pub enum Focus {
    Window(i64),
    /// The empty workspace past the end, where the launcher is the desktop.
    Home(u8),
}

/// A window that owns a workspace. Dialogs float and go with their app. A
/// portal's file chooser is floated by a sway rule, but may map before it.
fn tiles(window: &Window) -> bool {
    !window.floating && window.workspace > 0 && !window.app.starts_with("xdg-desktop-portal")
}

fn slot(index: usize) -> u8 {
    u8::try_from(index + 1).unwrap_or(u8::MAX)
}

/// Bring `order` up to date with the windows sway has, and list the moves that
/// put window `i` on workspace `i + 1`.
///
/// `windows` must include windows that were asked to close but have not gone
/// yet. One that refuses to close keeps its place.
pub fn plan(order: &[i64], windows: &[Window]) -> Plan {
    let tiled: Vec<&Window> = windows.iter().filter(|w| tiles(w)).collect();
    let mut new_order: Vec<i64> = order
        .iter()
        .copied()
        .filter(|id| tiled.iter().any(|w| w.id == *id))
        .collect();
    // In the order sway has them, so a daemon restart keeps the layout.
    let mut fresh: Vec<&Window> = tiled
        .iter()
        .copied()
        .filter(|w| !order.contains(&w.id))
        .collect();
    fresh.sort_by_key(|w| (w.workspace, w.id));
    let added: Vec<i64> = fresh.iter().map(|w| w.id).collect();
    new_order.extend(&added);

    let mut moves = Vec::new();
    for (index, id) in new_order.iter().enumerate() {
        let Some(window) = tiled.iter().find(|w| w.id == *id) else {
            continue;
        };
        let target = slot(index);
        if window.workspace != target {
            moves.push((window.id, target));
        }
    }
    Plan {
        order: new_order,
        moves: with_dialogs(windows, moves),
        added,
    }
}

/// Moves for going back to fixed workspaces. Each app's first window goes to
/// the workspace its profile names. Every other window keeps its workspace if
/// that is free, or takes the first free one.
pub fn fixed_moves(windows: &[Window], home_of: impl Fn(&Window) -> Option<u8>) -> Vec<(i64, u8)> {
    const SLOTS: std::ops::RangeInclusive<u8> = 1..=9;
    let tiled: Vec<&Window> = windows.iter().filter(|w| tiles(w)).collect();
    let mut taken: Vec<u8> = Vec::new();
    let mut placed: Vec<i64> = Vec::new();
    let mut moves = Vec::new();
    for window in &tiled {
        let Some(home) = home_of(window).filter(|n| SLOTS.contains(n)) else {
            continue;
        };
        if taken.contains(&home) {
            continue;
        }
        taken.push(home);
        placed.push(window.id);
        if window.workspace != home {
            moves.push((window.id, home));
        }
    }
    let mut homeless = Vec::new();
    for window in tiled.iter().filter(|w| !placed.contains(&w.id)) {
        if SLOTS.contains(&window.workspace) && !taken.contains(&window.workspace) {
            taken.push(window.workspace);
        } else {
            homeless.push(window.id);
        }
    }
    for id in homeless {
        let Some(free) = SLOTS.clone().find(|n| !taken.contains(n)) else {
            break;
        };
        taken.push(free);
        moves.push((id, free));
    }
    with_dialogs(windows, moves)
}

/// Add each moved window's dialogs to the moves. Only for a window alone on its
/// workspace: a window opened over another app leaves the dialogs with the app
/// that was there first.
fn with_dialogs(windows: &[Window], moves: Vec<(i64, u8)>) -> Vec<(i64, u8)> {
    let mut out = Vec::new();
    for (id, target) in moves {
        out.push((id, target));
        let Some(window) = windows.iter().find(|w| w.id == id) else {
            continue;
        };
        let alone = windows
            .iter()
            .filter(|w| tiles(w) && w.workspace == window.workspace)
            .count()
            == 1;
        if alone {
            out.extend(
                windows
                    .iter()
                    .filter(|w| w.floating && w.workspace == window.workspace)
                    .map(|w| (w.id, target)),
            );
        }
    }
    out
}

/// Where focus should go after `plan`, or `None` to leave it.
///
/// `front` is the tiled window that was on the focused workspace before the
/// moves, and `workspace` the focused workspace. `settled` is false on the
/// first plan after a start, when every window counts as new.
pub fn focus_after(plan: &Plan, front: Option<i64>, workspace: u8, settled: bool) -> Option<Focus> {
    let moved = |id: i64| plan.moves.iter().any(|(m, _)| *m == id);
    // A new window that had to be moved to the end is the app just opened.
    if settled {
        if let Some(id) = plan.added.iter().rev().find(|id| moved(**id)) {
            return Some(Focus::Window(*id));
        }
    }
    match front {
        // The window in front shifted down: follow it.
        Some(id) if moved(id) => Some(Focus::Window(id)),
        Some(_) => None,
        // The window in front has gone, and the next one would slide in.
        // Closing an app lands on the launcher instead.
        None if workspace >= 1 && usize::from(workspace) <= plan.order.len() => {
            Some(Focus::Home(slot(plan.order.len())))
        }
        None => None,
    }
}

/// Swap a window with its neighbour. False at either end, or for a window not
/// in the order.
pub fn shift(order: &mut [i64], id: i64, left: bool) -> bool {
    let Some(index) = order.iter().position(|w| *w == id) else {
        return false;
    };
    let other = if left {
        index.checked_sub(1)
    } else {
        (index + 1 < order.len()).then_some(index + 1)
    };
    match other {
        Some(other) => {
            order.swap(index, other);
            true
        }
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tiled(id: i64, workspace: u8) -> Window {
        Window {
            id,
            workspace,
            app: format!("app{id}"),
            title: String::new(),
            focused: false,
            floating: false,
        }
    }

    fn dialog(id: i64, workspace: u8) -> Window {
        Window {
            floating: true,
            ..tiled(id, workspace)
        }
    }

    #[test]
    fn a_settled_layout_needs_nothing() {
        let windows = [tiled(10, 1), tiled(11, 2), tiled(12, 3)];
        let plan = plan(&[10, 11, 12], &windows);
        assert_eq!(plan.order, [10, 11, 12]);
        assert!(plan.moves.is_empty());
        assert!(plan.added.is_empty());
    }

    #[test]
    fn a_first_look_keeps_the_layout_and_closes_the_gaps() {
        let windows = [tiled(12, 9), tiled(10, 2), tiled(11, 5)];
        let plan = plan(&[], &windows);
        assert_eq!(plan.order, [10, 11, 12]);
        assert_eq!(plan.moves, [(10, 1), (11, 2), (12, 3)]);
    }

    #[test]
    fn a_new_window_goes_on_the_end() {
        // Opened over the app on 1, sharing its workspace.
        let windows = [tiled(10, 1), tiled(11, 2), tiled(20, 1)];
        let plan = plan(&[10, 11], &windows);
        assert_eq!(plan.order, [10, 11, 20]);
        assert_eq!(plan.moves, [(20, 3)]);
        assert_eq!(plan.added, [20]);
        assert_eq!(
            focus_after(&plan, Some(10), 1, true),
            Some(Focus::Window(20))
        );
    }

    #[test]
    fn a_window_opened_on_home_is_already_in_place() {
        let windows = [tiled(10, 1), tiled(11, 2), tiled(20, 3)];
        let plan = plan(&[10, 11], &windows);
        assert!(plan.moves.is_empty());
        assert_eq!(focus_after(&plan, Some(20), 3, true), None);
    }

    #[test]
    fn closing_shifts_the_rest_down_and_lands_on_home() {
        // 11 on workspace 2 has gone, and 2 was in front.
        let windows = [tiled(10, 1), tiled(12, 3), tiled(13, 4)];
        let plan = plan(&[10, 11, 12, 13], &windows);
        assert_eq!(plan.order, [10, 12, 13]);
        assert_eq!(plan.moves, [(12, 2), (13, 3)]);
        assert_eq!(focus_after(&plan, None, 2, true), Some(Focus::Home(4)));
    }

    #[test]
    fn closing_one_behind_follows_the_window_in_front() {
        // From the switcher: 10 closed while 12 was in front on 3.
        let windows = [tiled(11, 2), tiled(12, 3)];
        let plan = plan(&[10, 11, 12], &windows);
        assert_eq!(plan.moves, [(11, 1), (12, 2)]);
        assert_eq!(
            focus_after(&plan, Some(12), 3, true),
            Some(Focus::Window(12))
        );
    }

    #[test]
    fn closing_the_last_one_stays_on_home() {
        let windows = [tiled(10, 1), tiled(11, 2)];
        let plan = plan(&[10, 11, 12], &windows);
        assert!(plan.moves.is_empty());
        // Workspace 3 is past the end now: it is home already.
        assert_eq!(focus_after(&plan, None, 3, true), None);
    }

    #[test]
    fn a_window_refusing_to_close_keeps_its_place() {
        // Still in the tree while it asks about unsaved work.
        let windows = [tiled(10, 1), tiled(11, 2), tiled(12, 3)];
        let plan = plan(&[10, 11, 12], &windows);
        assert!(plan.moves.is_empty());
    }

    #[test]
    fn dialogs_go_with_their_app() {
        let windows = [tiled(10, 1), tiled(12, 3), dialog(30, 3)];
        let plan = plan(&[10, 11, 12], &windows);
        assert_eq!(plan.moves, [(12, 2), (30, 2)]);
    }

    #[test]
    fn a_new_window_leaves_the_dialogs_behind() {
        let windows = [tiled(10, 1), dialog(30, 1), tiled(20, 1)];
        let plan = plan(&[10], &windows);
        assert_eq!(plan.moves, [(20, 2)]);
    }

    #[test]
    fn nothing_moves_into_focus_on_a_restart() {
        let windows = [tiled(10, 1), tiled(20, 1)];
        let plan = plan(&[], &windows);
        assert_eq!(plan.moves, [(20, 2)]);
        assert_eq!(focus_after(&plan, Some(10), 1, false), None);
    }

    #[test]
    fn a_swap_moves_both_and_focus_follows() {
        let mut order = vec![10, 11, 12];
        assert!(shift(&mut order, 12, true));
        assert_eq!(order, [10, 12, 11]);
        let windows = [tiled(10, 1), tiled(11, 2), tiled(12, 3)];
        let plan = plan(&order, &windows);
        assert_eq!(plan.moves, [(12, 2), (11, 3)]);
        assert_eq!(
            focus_after(&plan, Some(12), 3, true),
            Some(Focus::Window(12))
        );
    }

    #[test]
    fn shift_stops_at_the_ends() {
        let mut order = vec![10, 11];
        assert!(!shift(&mut order, 10, true));
        assert!(!shift(&mut order, 11, false));
        assert!(!shift(&mut order, 99, false));
        assert_eq!(order, [10, 11]);
    }

    /// Profiles for the fixed tests: app10 owns 2, app11 owns 1.
    fn home(w: &Window) -> Option<u8> {
        match w.app.as_str() {
            "app10" => Some(2),
            "app11" => Some(1),
            _ => None,
        }
    }

    #[test]
    fn going_fixed_sends_each_app_home() {
        let windows = [tiled(10, 1), tiled(11, 2), tiled(12, 3)];
        assert_eq!(fixed_moves(&windows, home), [(10, 2), (11, 1)]);
    }

    #[test]
    fn going_fixed_moves_an_app_out_of_another_apps_slot() {
        // 12 has no profile and sits on 2, which belongs to 10.
        let windows = [tiled(12, 2), tiled(10, 5)];
        assert_eq!(fixed_moves(&windows, home), [(10, 2), (12, 1)]);
    }

    #[test]
    fn going_fixed_keeps_a_free_workspace_and_rescues_high_ones() {
        let windows = [tiled(12, 4), tiled(13, 12), tiled(10, 2)];
        assert_eq!(fixed_moves(&windows, home), [(13, 1)]);
    }

    #[test]
    fn going_fixed_sends_only_an_apps_first_window_home() {
        // A second window of app10, on 2 with the first.
        let mut second = tiled(20, 3);
        second.app = "app10".into();
        let windows = [tiled(10, 2), second];
        assert!(fixed_moves(&windows, home).is_empty());
    }

    #[test]
    fn going_fixed_takes_the_dialogs_along() {
        let windows = [tiled(10, 4), dialog(30, 4)];
        assert_eq!(fixed_moves(&windows, home), [(10, 2), (30, 2)]);
    }

    #[test]
    fn floating_and_portal_windows_take_no_slot() {
        let mut portal = tiled(40, 2);
        portal.app = "xdg-desktop-portal-gtk".into();
        let windows = [tiled(10, 1), dialog(30, 5), portal];
        let plan = plan(&[10], &windows);
        assert_eq!(plan.order, [10]);
        assert!(plan.moves.is_empty());
    }
}

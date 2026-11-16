//! Stateless policy for resolved Ghostty actions. No native catalog or geometry.
use super::action::{WorkspaceLifecycleDecision, WorkspaceLifecycleInput};

pub(super) fn decide(input: &WorkspaceLifecycleInput) -> WorkspaceLifecycleDecision {
    let mut result = WorkspaceLifecycleDecision {
        effect: "unhandled".into(),
        selected: input.selected.clone(),
        error: None,
        can_archive: false,
    };
    let action = input.action.strip_prefix("native.").unwrap_or_default();
    let structural = matches!(
        action,
        "new_tab"
            | "close_tab"
            | "close_other_tabs"
            | "close_right_tabs"
            | "close_all_tabs"
            | "rename_tab"
            | "move_tab"
            | "close_pane"
    );
    let disabled = matches!(
        action,
        "split"
            | "resize_split"
            | "equalize_splits"
            | "zoom_split"
            | "undo"
            | "redo"
            | "merge_windows"
            | "reopen_tab"
    );
    if !structural && !disabled {
        return result;
    }
    let unavailable = input
        .bound
        .as_ref()
        .is_none_or(|id| !input.ids.contains(id) || input.archived.contains(id))
        || input.tab_count == 0
        || (action != "new_tab" && input.tab_index.is_none_or(|index| index >= input.tab_count))
        || (action == "close_other_tabs" && input.tab_count <= 1)
        || (action == "close_right_tabs"
            && input
                .tab_index
                .is_none_or(|index| index + 1 >= input.tab_count));
    if disabled || unavailable {
        result.effect = "disabled".into();
        result.error = Some(if disabled { "This native layout action is unavailable in Station. Use the Workspace layout controls." } else { "The originating Workspace or Tab is unavailable." }.into());
    } else {
        result.effect =
            if action == "close_all_tabs" || (action == "close_tab" && input.tab_count == 1) {
                "close"
            } else {
                action
            }
            .into();
    }
    result
}

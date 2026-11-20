//! One catalog edit shared by the native Settings Pane and other local callers.
use crate::MAX_PANES_PER_TAB;
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::BTreeMap;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Layout {
    name: String,
    tabs: Vec<Tab>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Tab {
    id: String,
    title: String,
    pane_ids: Vec<String>,
}

/// Operations on persisted native IDs, including older non-UUID layouts.
#[derive(Deserialize)]
#[serde(tag = "intent", rename_all = "snake_case", deny_unknown_fields)]
pub enum Operation {
    RenameTab {
        tab_id: String,
        title: String,
    },
    ReorderTab {
        tab_id: String,
        to_index: usize,
    },
    ReorderPane {
        tab_id: String,
        pane_id: String,
        to_index: usize,
    },
    MovePane {
        from_tab_id: String,
        to_tab_id: String,
        pane_id: String,
        to_index: usize,
    },
}

fn array<'a>(value: &'a Value, key: &str) -> Result<&'a [Value], String> {
    value[key]
        .as_array()
        .map(Vec::as_slice)
        .ok_or_else(|| format!("Missing {key}"))
}

fn id(value: &Value) -> Result<&str, String> {
    value["id"]
        .as_str()
        .filter(|id| !id.is_empty())
        .ok_or_else(|| "Missing layout ID".into())
}

fn checked_font_points(points: i64) -> Result<u8, String> {
    use crate::{MAX_MANAGED_TERMINAL_FONT_SIZE_POINTS as MAX, MIN_MANAGED_TERMINAL_FONT_SIZE_POINTS as MIN};
    if !(i64::from(MIN)..=i64::from(MAX)).contains(&points) {
        return Err(format!("Enter a terminal font size from {MIN} to {MAX} points"));
    }
    u8::try_from(points).map_err(|error| error.to_string())
}

pub(crate) fn font_preference(workspace_id: &str, points: i64) -> Result<Value, String> {
    if workspace_id.is_empty() {
        return Err("Open a Workspace to edit its terminal font".into());
    }
    Ok(json!({
        "workspace_id": workspace_id,
        "points": checked_font_points(points)?,
        "min": crate::MIN_MANAGED_TERMINAL_FONT_SIZE_POINTS,
        "max": crate::MAX_MANAGED_TERMINAL_FONT_SIZE_POINTS,
    }))
}

pub(crate) fn set_font(mut catalog: Value, workspace_id: &str, points: i64) -> Result<Value, String> {
    let points = checked_font_points(points)?;
    let index = workspace_index(&catalog, workspace_id)?;
    if catalog["archived_workspace_ids"].as_array().is_some_and(|ids| ids.contains(&json!(workspace_id))) {
        return Err("Workspace is archived".into());
    }
    let workspace = &mut catalog["workspaces"][index];
    if workspace["managed_terminal_font_size_points"] != json!(points) {
        let revision = workspace["revision"].as_u64().and_then(|value| value.checked_add(1))
            .ok_or("Invalid Workspace revision")?;
        workspace["managed_terminal_font_size_points"] = json!(points);
        workspace["revision"] = json!(revision);
    }
    Ok(catalog)
}

pub(crate) fn directory(catalog: &Value) -> Result<Value, String> {
    let archived = catalog["archived_workspace_ids"].as_array();
    Ok(json!({"rows": Value::Array(
        array(catalog, "workspaces")?
            .iter()
            .map(|workspace| {
                json!({
                    "id":workspace["id"], "name":workspace["name"],
                    "archived":archived.is_some_and(|ids| ids.contains(&workspace["id"]))
                })
            })
            .collect(),
    )}))
}

fn workspace_index(catalog: &Value, workspace_id: &str) -> Result<usize, String> {
    array(catalog, "workspaces")?
        .iter()
        .position(|w| w["id"] == workspace_id)
        .ok_or_else(|| "Workspace no longer exists".into())
}

pub(crate) fn detail(catalog: &Value, workspace_id: &str) -> Result<Value, String> {
    let workspace = &catalog["workspaces"][workspace_index(catalog, workspace_id)?];
    describe(workspace, catalog["catalog_revision"].clone())
}

fn describe(workspace: &Value, revision: Value) -> Result<Value, String> {
    let tabs = array(workspace, "tabs")?
        .iter()
        .map(|tab| {
            json!({
                "id":tab["id"], "title":tab["title"], "pane_ids":tab["visual_pane_ids"]
            })
        })
        .collect::<Vec<_>>();
    Ok(json!({
        "revision":revision,
        "workspace":workspace,
        "layout":{"name":workspace["name"],"tabs":tabs},
    }))
}

fn take_by_id(rows: &[Value]) -> Result<BTreeMap<String, Value>, String> {
    let mut by_id = BTreeMap::new();
    for row in rows {
        if by_id.insert(id(row)?.to_owned(), row.clone()).is_some() {
            return Err("Duplicate layout ID".into());
        }
    }
    Ok(by_id)
}

fn label(value: &str) -> Result<&str, String> {
    let value = value.trim();
    if value.is_empty() || value.chars().any(char::is_control) || value.len() > 256 {
        return Err("Enter a name of 1–256 bytes without control characters".into());
    }
    Ok(value)
}

pub(crate) fn update(
    mut catalog: Value,
    workspace_id: &str,
    revision: u64,
    layout: Layout,
) -> Result<Value, String> {
    if catalog["catalog_revision"].as_u64() != Some(revision) {
        return Err(format!(
            "Workspace changed. Reload revision {} before saving.",
            catalog["catalog_revision"]
        ));
    }
    let index = workspace_index(&catalog, workspace_id)?;
    apply_layout(&mut catalog["workspaces"][index], layout)?;
    // The existing serialized writer advances the catalog revision after its own check.
    Ok(catalog)
}

pub(crate) fn edit(
    mut base: Value,
    workspace_id: &str,
    revision: u64,
    mut layout: Layout,
    operation: Operation,
) -> Result<Value, String> {
    if id(&base)? != workspace_id {
        return Err("The edit belongs to a different Workspace".into());
    }
    layout.apply(operation)?;
    apply_layout(&mut base, layout)?;
    describe(&base, json!(revision))
}

impl Layout {
    fn tab_index(&self, id: &str) -> Result<usize, String> {
        self.tabs
            .iter()
            .position(|tab| tab.id == id)
            .ok_or_else(|| "Tab is not in this Workspace".into())
    }

    fn apply(&mut self, operation: Operation) -> Result<(), String> {
        match operation {
            Operation::RenameTab { tab_id, title } => {
                let index = self.tab_index(tab_id.as_str())?;
                self.tabs[index].title = title;
            }
            Operation::ReorderTab { tab_id, to_index } => {
                let index = self.tab_index(tab_id.as_str())?;
                if to_index >= self.tabs.len() {
                    return Err("Tab position is outside this Workspace".into());
                }
                let tab = self.tabs.remove(index);
                self.tabs.insert(to_index, tab);
            }
            Operation::ReorderPane {
                tab_id,
                pane_id,
                to_index,
            } => {
                self.move_pane(tab_id.as_str(), tab_id.as_str(), pane_id.as_str(), to_index)?;
            }
            Operation::MovePane {
                from_tab_id,
                to_tab_id,
                pane_id,
                to_index,
            } => {
                self.move_pane(
                    from_tab_id.as_str(),
                    to_tab_id.as_str(),
                    pane_id.as_str(),
                    to_index,
                )?;
            }
        }
        Ok(())
    }

    fn move_pane(
        &mut self,
        from: &str,
        to: &str,
        pane: &str,
        position: usize,
    ) -> Result<(), String> {
        let source = self.tab_index(from)?;
        let target = self.tab_index(to)?;
        let index = self.tabs[source]
            .pane_ids
            .iter()
            .position(|id| id == pane)
            .ok_or("Pane is not in the requested source Tab")?;
        let destination_count = self.tabs[target].pane_ids.len();
        if source != target && destination_count >= MAX_PANES_PER_TAB {
            return Err("A Tab can hold at most nine Panes".into());
        }
        if position > destination_count - usize::from(source == target) {
            return Err("Pane position is outside the target Tab".into());
        }
        let pane = self.tabs[source].pane_ids.remove(index);
        self.tabs[target].pane_ids.insert(position, pane);
        Ok(())
    }
}

fn apply_layout(workspace: &mut Value, layout: Layout) -> Result<(), String> {
    let name = if workspace["name"] == layout.name {
        layout.name.as_str()
    } else {
        label(&layout.name)?
    };
    let original = array(workspace, "tabs")?;
    let mut tabs = take_by_id(original)?;
    let panes = original
        .iter()
        .map(|tab| array(tab, "panes"))
        .collect::<Result<Vec<_>, _>>()?;
    let mut panes = take_by_id(&panes.concat())?;
    let mut ordered = Vec::with_capacity(tabs.len());
    for edit in layout.tabs {
        let mut tab = tabs.remove(&edit.id).ok_or("Unknown or repeated Tab ID")?;
        if edit.pane_ids.len() > MAX_PANES_PER_TAB {
            return Err("A Tab can hold at most nine Panes".into());
        }
        if tab["title"] != edit.title {
            tab["title"] = json!(label(&edit.title)?);
        }
        let mut visual = edit.pane_ids;
        if tab["visual_pane_ids"] != json!(visual)
            && (tab["display_mode"] == "focus" || tab["mode"] == "focus")
        {
            if let Some(main) = tab["main_pane_id"]
                .as_str()
                .and_then(|main| visual.iter().position(|id| id == main))
            {
                visual[..=main].rotate_right(1);
            }
            tab["main_pane_id"] = json!(visual.first());
        }
        let mut ordered_panes = Vec::with_capacity(visual.len());
        for id in &visual {
            ordered_panes.push(panes.remove(id).ok_or("Unknown or repeated Pane ID")?);
        }
        let visual = json!(visual);
        if tab["visual_pane_ids"] != visual {
            tab["visual_pane_ids"] = visual;
            if let Some(canonical) = tab.get_mut("canonical_pane_ids") {
                *canonical = json!(ordered_panes.iter().map(|p| &p["id"]).collect::<Vec<_>>());
            }
            tab.as_object_mut()
                .expect("validated Tab")
                .remove("split_ratios");
            tab["panes"] = json!(ordered_panes);
        }
        ordered.push(tab);
    }
    if !tabs.is_empty() || !panes.is_empty() {
        return Err("Include every original Tab and Pane".into());
    }
    if workspace["name"] == name && array(workspace, "tabs")? == ordered {
        return Ok(());
    }
    workspace["name"] = json!(name);
    workspace["tabs"] = json!(ordered);
    let selected = &workspace["selected_pane_id"];
    if !selected.is_null()
        && !ordered.iter().any(|tab| {
            tab["id"] == workspace["selected_tab_id"]
                && tab["visual_pane_ids"]
                    .as_array()
                    .is_some_and(|ids| ids.contains(selected))
        })
    {
        workspace
            .as_object_mut()
            .expect("validated Workspace")
            .remove("selected_pane_id");
    }
    let next = workspace["revision"]
        .as_u64()
        .and_then(|value| value.checked_add(1))
        .ok_or("Invalid Workspace revision")?;
    workspace["revision"] = json!(next);
    Ok(())
}






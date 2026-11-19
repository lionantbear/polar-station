//! One catalog edit shared by the native Settings Pane and other local callers.
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

pub(super) fn directory(catalog: &Value) -> Result<Value, String> {
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

pub(super) fn detail(catalog: &Value, workspace_id: &str) -> Result<Value, String> {
    let workspace = &catalog["workspaces"][workspace_index(catalog, workspace_id)?];
    let tabs = array(workspace, "tabs")?
        .iter()
        .map(|tab| {
            json!({
                "id":tab["id"], "title":tab["title"], "pane_ids":tab["visual_pane_ids"]
            })
        })
        .collect::<Vec<_>>();
    Ok(json!({
        "revision":catalog["catalog_revision"],
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

pub(super) fn update(
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
    let workspace = &mut catalog["workspaces"][index];
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
        if edit.pane_ids.len() > 9 {
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
            tab["canonical_pane_ids"] =
                json!(ordered_panes.iter().map(|p| &p["id"]).collect::<Vec<_>>());
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
        return Ok(catalog);
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
    // The existing serialized writer advances the catalog revision after its own check.
    Ok(catalog)
}



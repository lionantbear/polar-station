//! Local-only native file adapters. No native structure is sent to a Host.
use super::*;
mod settings;
use polar_station_protocol::{PaneContentBindingV1, PolarConnectionV1, PortableHostV1};
use serde_json::{json, Value};

#[derive(Deserialize, Serialize)]
struct SavedWorkspace {
    id: String,
    name: String,
    selected_tab_id: String,
    managed_terminal_font_size_points: u8,
    tabs: Vec<SavedTab>,
    #[serde(default)]
    portable_hosts: Option<Vec<PortableHostV1>>,
    #[serde(default)]
    portable_source_workspace_id: Option<String>,
}
#[derive(Deserialize, Serialize)]
struct SavedTab {
    id: String,
    title: String,
    #[serde(alias = "mode")]
    display_mode: Mode,
    main_pane_id: Option<String>,
    #[serde(default)]
    canonical_pane_ids: Vec<String>,
    visual_pane_ids: Vec<String>,
    #[serde(default)]
    split_ratios: Option<Vec<f64>>,
    panes: Vec<SavedPane>,
}
#[derive(Deserialize, Serialize)]
struct SavedPane {
    id: String,
    execution_target_id: Option<String>,
    tmux_session_id: Option<String>,
    content: Option<StationPaneContentV1>,
    source: Option<TerminalSourceV1>,
}
#[derive(Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum Request {
    WorkspaceDirectory { catalog: Value },
    WorkspaceDetails { catalog: Value, workspace_id: String },
    UpdateWorkspace { catalog: Value, workspace_id: String, revision: u64, layout: settings::Layout },
    PrepareCatalog {catalog:Value},
    ProtectCatalog {catalog:Value,bundle:HostExportBundleV1,sources:Vec<TerminalSourceV1>},
    ResolveCatalog {catalog:Value,associations:Vec<HostImportAssociationV1>,#[serde(default)] polar_instances:Vec<PolarConnectionV1>},
    PrepareExport {
        native: Value,
    },
    ResolveNative {
        native: Value,
        associations: Vec<HostImportAssociationV1>,
        #[serde(default)]
        polar_instances: Vec<PolarConnectionV1>,
    },
    Sources {
        native: Value,
    },
    Export {
        native: Value,
        bundle: HostExportBundleV1,
        sources: Vec<TerminalSourceV1>,
        #[serde(default)]
        idea_sources: BTreeMap<String, Vec<String>>,
        source_workspace: String,
    },
    Inspect {
        package: Package,
    },
    Import {
        package: Package,
        name: String,
        associations: Vec<HostImportAssociationV1>,
        #[serde(default)]
        polar_instances: Vec<PolarConnectionV1>,
        platform: String,
    },
    ReferenceView {
        reference: Content,
    },
}

fn native(value: Value) -> Result<SavedWorkspace, String> {
    serde_json::from_value(value).map_err(|e| format!("Invalid native Workspace: {e}"))
}
pub fn dispatch(request: Request) -> Result<Value, String> {
    match request {
        Request::WorkspaceDirectory { catalog } => settings::directory(&catalog),
        Request::WorkspaceDetails { catalog, workspace_id } => settings::detail(&catalog, &workspace_id),
        Request::UpdateWorkspace { catalog, workspace_id, revision, layout } => settings::update(catalog, &workspace_id, revision, layout),
        Request::PrepareCatalog {catalog}=>{
            let workspaces=catalog.get("workspaces").and_then(Value::as_array).ok_or("Missing Workspace catalog")?;
            let mut bindings=Vec::new();let mut targets=BTreeSet::new();let mut sources=BTreeSet::new();let mut references=Vec::new();let mut panes=0;
            for workspace in workspaces {
                let saved=native(workspace.clone())?;
                panes+=saved.tabs.iter().map(|t|t.panes.len()).sum::<usize>();
                for tab in &saved.tabs {for (index,pane) in tab.panes.iter().enumerate(){if let Some(source)=&pane.source{
                    sources.insert(source.host.clone());
                    let host=saved.portable_hosts.iter().flatten().find(|h|h.source==source.host).map(|h|h.display_name.as_str()).unwrap_or(&source.host.target_id);
                    references.push(json!({"source":source.host,"title":format!("{} / {} / Pane {} · {}",saved.name,tab.title,index+1,host)}));
                }}}
                let result=dispatch(Request::PrepareExport {native:workspace.clone()})?;
                bindings.extend(result["bindings"].as_array().ok_or("Invalid bindings")?.clone());
                for target in result["target_ids"].as_array().ok_or("Invalid targets")? {targets.insert(target.as_str().ok_or("Invalid target")?.to_owned());}
            }
            if bindings.len()>1152 {return Err("This catalog has too many unresolved terminal references for one switch; export individual Workspaces first".into());}
            Ok(json!({"bindings":bindings,"target_ids":targets,"sources":sources,"references":references,"workspace_count":workspaces.len(),"pane_count":panes}))
        },
        Request::ProtectCatalog {mut catalog,bundle,sources}=>{
            if bundle.schema!=polar_station_protocol::HOST_EXPORT_SCHEMA_V1 || bundle.hosts.len()>256 {return Err("Invalid Host export bundle".into());}
            for host in &bundle.hosts {host.source.validate().map_err(|e|e.to_string())?;}
            let mut sources=sources.into_iter();
            for workspace in catalog.get_mut("workspaces").and_then(Value::as_array_mut).ok_or("Missing Workspace catalog")? {
                let saved=native(workspace.clone())?;
                let mut hosts=saved.portable_hosts.unwrap_or_default();
                for host in &bundle.hosts {if !hosts.iter().any(|h|h.source==host.source){hosts.push(host.clone());}}
                for tab in workspace.get_mut("tabs").and_then(Value::as_array_mut).ok_or("Missing Tabs")? {
                    for pane in tab.get_mut("panes").and_then(Value::as_array_mut).ok_or("Missing Panes")? {
                        let parsed:SavedPane=serde_json::from_value(pane.clone()).map_err(|e|e.to_string())?;
                        match parsed.content {
                            None if parsed.source.is_none()=>{
                                let source=sources.next().ok_or("Missing original terminal observation")?;
                                source.validate().map_err(|e|e.to_string())?;
                                let target=parsed.execution_target_id.as_deref().ok_or("Missing terminal target")?;
                                if !bundle.hosts.iter().any(|h|h.source==source.host && h.source.target_id==target){return Err("Terminal observation and Host source differ".into());}
                                pane["source"]=serde_json::to_value(source).map_err(|e|e.to_string())?;
                            },
                            Some(StationPaneContentV1::ToolSetup {setup_id,execution_target_id})=>{
                                let host=bundle.hosts.iter().find(|h|h.source.target_id==execution_target_id).ok_or("Missing setup source")?;
                                pane["content"]=json!({"kind":"unavailable","reference":Content::ToolSetup {setup_id,host:host.source.clone()}});
                            },
                            _=>{},
                        }
                    }
                }
                workspace["portable_hosts"]=serde_json::to_value(hosts).map_err(|e|e.to_string())?;
            }
            if sources.next().is_some(){return Err("Unexpected terminal observations".into());}
            Ok(catalog)
        },
        Request::ResolveCatalog {mut catalog,associations,polar_instances}=>{
            for workspace in catalog.get_mut("workspaces").and_then(Value::as_array_mut).ok_or("Missing Workspace catalog")? {
                let result=dispatch(Request::ResolveNative {native:workspace.clone(),associations:associations.clone(),polar_instances:polar_instances.clone()})?;
                *workspace=result["native"].clone();
            }
            Ok(catalog)
        },
        Request::Sources { native: value } => {
            let saved = native(value)?;
            let sources = saved
                .tabs
                .iter()
                .flat_map(|t| &t.panes)
                .filter_map(|p| p.source.as_ref().map(|s| s.host.clone()))
                .collect::<BTreeSet<_>>();
            let needed=!sources.is_empty() || saved.tabs.iter().flat_map(|t|&t.panes).any(|p|matches!(&p.content,Some(StationPaneContentV1::Unavailable{reference}) if matches!(reference.as_ref(),Content::Lgp{..})));
            Ok(json!({"sources":sources,"needed":needed}))
        }
        Request::ResolveNative {
            mut native,
            associations,
            polar_instances,
        } => {
            let original = native.clone();
            for association in &associations {
                association.source.validate().map_err(|e| e.to_string())?;
                id(&association.target_id)?;
            }
            for tab in native
                .get_mut("tabs")
                .and_then(Value::as_array_mut)
                .ok_or("Missing Tabs")?
            {
                for pane in tab
                    .get_mut("panes")
                    .and_then(Value::as_array_mut)
                    .ok_or("Missing Panes")?
                {
                    if let Some(source) = pane.get("source").filter(|v| !v.is_null()) {
                        let source: TerminalSourceV1 =
                            serde_json::from_value(source.clone()).map_err(|e| e.to_string())?;
                        source.validate().map_err(|e| e.to_string())?;
                        if let Some(association) =
                            associations.iter().find(|a| a.source == source.host)
                        {
                            pane["execution_target_id"] = json!(association.target_id);
                        }
                    }
                    if pane["content"]["kind"] == "lgp_text" {
                        let content:StationPaneContentV1=serde_json::from_value(pane["content"].clone()).map_err(|e|e.to_string())?;
                        if let StationPaneContentV1::LgpText {node_id,owner,thread,actor,..}=content {
                            pane["content"]=json!(StationPaneContentV1::Unavailable {reference:Box::new(Content::Lgp {node_id,owner,thread,actor})});
                        }
                    }
                    if pane["content"]["kind"] == "unavailable" {
                        let reference: Content =
                            serde_json::from_value(pane["content"]["reference"].clone())
                                .map_err(|e| e.to_string())?;
                        if let Content::Lgp {
                            node_id,
                            owner,
                            thread,
                            actor,
                        } = reference
                        {
                            let matches = polar_instances
                                .iter()
                                .filter(|p| p.expected_node_id == node_id)
                                .collect::<Vec<_>>();
                            if matches.len() == 1 {
                                pane["content"] = json!(StationPaneContentV1::LgpText {
                                    target_id: matches[0].id.clone(),
                                    node_id,
                                    owner,
                                    thread,
                                    actor
                                });
                            }
                        }
                    }
                }
            }
            Ok(json!({"changed":native!=original,"native":native}))
        }
        Request::PrepareExport { native: value } => {
            let native = native(value)?;
            let mut bindings = Vec::new();
            let mut target_ids = BTreeSet::new();
            for pane in native.tabs.iter().flat_map(|t| &t.panes) {
                if pane.content.is_none() && pane.source.is_none() {
                    let target = pane
                        .execution_target_id
                        .as_ref()
                        .ok_or("Terminal Host is missing")?;
                    let session = pane
                        .tmux_session_id
                        .as_ref()
                        .ok_or("Terminal Session is missing")?;
                    bindings.push(PaneContentBindingV1 {
                        execution_target: polar_station_protocol::ExecutionTarget::new(target)
                            .map_err(|e| e.to_string())?,
                        tmux_session_id: session.clone(),
                        source: None,
                    });
                }
                if let Some(StationPaneContentV1::ToolSetup {
                    execution_target_id,
                    ..
                }) = &pane.content
                {
                    target_ids.insert(execution_target_id);
                }
            }
            Ok(json!({"bindings":bindings,"target_ids":target_ids}))
        }
        Request::Export {
            native: value,
            bundle,
            sources,
            idea_sources,
            source_workspace,
        } => {
            let native = native(value)?;
            id(&source_workspace)?;
            let mut sources = sources.into_iter();
            let mut hosts = native
                .portable_hosts
                .iter()
                .flatten()
                .map(|h| (h.source.clone(), h.clone()))
                .collect::<BTreeMap<_, _>>();
            for host in &bundle.hosts {
                hosts.insert(host.source.clone(), host.clone());
            }
            let mut tabs = Vec::new();
            for tab in native.tabs {
                let mut panes = Vec::new();
                for pane in tab.panes {
                    let content = match pane.content {
                        None => Content::Terminal {
                            source: pane
                                .source
                                .or_else(|| sources.next())
                                .ok_or("Missing exported terminal source")?,
                        },
                        Some(StationPaneContentV1::Unavailable { reference }) => *reference,
                        Some(StationPaneContentV1::LgpText {
                            node_id,
                            owner,
                            thread,
                            actor,
                            ..
                        }) => Content::Lgp {
                            node_id,
                            owner,
                            thread,
                            actor,
                        },
                        Some(StationPaneContentV1::IdeaTree { view_id }) => Content::IdeaTree {
                            source_workspace: source_workspace.clone(),
                            view_id: view_id.clone(),
                            paths: idea_sources.get(&view_id).cloned().unwrap_or_default(),
                        },
                        Some(StationPaneContentV1::SettingsPane {}) => Content::SettingsPane {},
                        Some(StationPaneContentV1::SettingsText { field }) => Content::Settings {
                            field: serde_json::to_value(field)
                                .map_err(|e| e.to_string())?
                                .as_str()
                                .ok_or("Invalid Settings field")?
                                .into(),
                        },
                        Some(StationPaneContentV1::ToolSetup {
                            execution_target_id,
                            setup_id,
                        }) => Content::ToolSetup {
                            host: bundle
                                .hosts
                                .iter()
                                .find(|h| h.source.target_id == execution_target_id)
                                .ok_or("Setup Host source is missing")?
                                .source
                                .clone(),
                            setup_id,
                        },
                        Some(_) => {
                            return Err(
                                "Legacy diagnostics must be reopened from Settings before export"
                                    .into(),
                            )
                        }
                    };
                    panes.push(Pane {
                        id: pane.id,
                        content,
                    });
                }
                let canonical_pane_ids = if tab.canonical_pane_ids.is_empty() {
                    panes.iter().map(|p| p.id.clone()).collect()
                } else {
                    tab.canonical_pane_ids
                };
                tabs.push(Tab {
                    id: tab.id,
                    title: tab.title,
                    display_mode: tab.display_mode,
                    main_pane_id: tab.main_pane_id,
                    canonical_pane_ids,
                    visual_pane_ids: tab.visual_pane_ids,
                    split_ratios: tab.split_ratios.unwrap_or_default(),
                    panes,
                });
            }
            if sources.next().is_some() {
                return Err("Export source count changed; retry export".into());
            }
            let required = tabs
                .iter()
                .flat_map(|t| &t.panes)
                .filter_map(|p| p.content.host())
                .cloned()
                .collect::<BTreeSet<_>>();
            let package = Package {
                schema: SCHEMA.into(),
                source_workspace_id: native
                    .portable_source_workspace_id
                    .unwrap_or_else(|| native.id.clone()),
                workspace: Workspace {
                    id: native.id,
                    name: native.name,
                    selected_tab_id: native.selected_tab_id,
                    managed_terminal_font_size_points: native.managed_terminal_font_size_points,
                    tabs,
                },
                hosts: HostExportBundleV1 {
                    schema: polar_station_protocol::HOST_EXPORT_SCHEMA_V1.into(),
                    hosts: hosts
                        .into_iter()
                        .filter(|(id, _)| required.contains(id))
                        .map(|(_, host)| host)
                        .collect(),
                },
            };
            package.validate()?;
            serde_json::to_value(package).map_err(|e| e.to_string())
        }
        Request::Inspect { package } => {
            package.validate()?;
            Ok(
                json!({"preview":package.preview(&[])?,"hosts":package.hosts,"name":format!("{} copy",package.workspace.name)}),
            )
        }
        Request::Import {
            package,
            name,
            associations,
            polar_instances,
            platform,
        } => {
            let copy = package.new_copy(&name)?;
            let mut preview = copy.preview(&associations)?;
            let initially_unavailable = preview.unavailable_contents;
            preview.unavailable_contents += copy
                .workspace
                .tabs
                .iter()
                .flat_map(|t| &t.panes)
                .filter(|p| match &p.content {
                    Content::Lgp { node_id, .. } => {
                        polar_instances
                            .iter()
                            .filter(|p| &p.expected_node_id == node_id)
                            .count()
                            != 1
                    }
                    _ => false,
                })
                .count();
            if preview.unavailable_contents > initially_unavailable {
                preview.summary.push_str(&format!(
                    "\n\n{} conversations need their original Polar connection.",
                    preview.unavailable_contents - initially_unavailable
                ));
            }
            if preview.unavailable_contents > 0 && preview.warning.is_none() {
                preview.warning=Some("Some Panes require their original content connection. Their references will be retained.".into());
            }
            let mut missing = BTreeMap::new();
            let mut tabs = Vec::new();
            for tab in &copy.workspace.tabs {
                let mut panes = Vec::new();
                for pane in &tab.panes {
                    let mut row = json!({"id":pane.id});
                    match &pane.content {
                        Content::Terminal { source } => {
                            let target = associations
                                .iter()
                                .find(|a| a.source == source.host)
                                .map(|a| a.target_id.clone())
                                .unwrap_or_else(|| {
                                    missing
                                        .entry(source.host.clone())
                                        .or_insert_with(|| {
                                            format!(
                                                "unresolved-host-{}",
                                                polar_identity::generation::new_uuid_v4()
                                            )
                                        })
                                        .clone()
                                });
                            row["execution_target_id"] = json!(target);
                            row["tmux_session_id"] = json!(source
                                .session
                                .as_ref()
                                .map_or("unresolved-session", |s| s.session_id.as_str()));
                            row["source"] = json!(source);
                        }
                        Content::Lgp {
                            node_id,
                            owner,
                            thread,
                            actor,
                        } => {
                            let targets = polar_instances
                                .iter()
                                .filter(|p| &p.expected_node_id == node_id)
                                .collect::<Vec<_>>();
                            row["content"] = if targets.len() == 1 {
                                json!(StationPaneContentV1::LgpText {
                                    target_id: targets[0].id.clone(),
                                    node_id: node_id.clone(),
                                    owner: owner.clone(),
                                    thread: thread.clone(),
                                    actor: actor.clone()
                                })
                            } else {
                                json!(StationPaneContentV1::Unavailable {
                                    reference: Box::new(pane.content.clone())
                                })
                            };
                        }
                        Content::SettingsPane {} => {
                            row["content"] = json!(StationPaneContentV1::SettingsPane {});
                        },
                        Content::Settings { field } => {
                            row["content"] = json!({"kind":"settings_text","field":field})
                        }
                        _ => {
                            row["content"] = json!(StationPaneContentV1::Unavailable {
                                reference: Box::new(pane.content.clone())
                            })
                        }
                    }
                    panes.push(row);
                }
                let mut row = json!({"id":tab.id,"title":tab.title,"display_mode":tab.display_mode,"main_pane_id":tab.main_pane_id,
                    "canonical_pane_ids":tab.canonical_pane_ids,"visual_pane_ids":tab.visual_pane_ids,"split_ratios":tab.split_ratios,"panes":panes});
                if platform == "macos" {
                    let mode = row.as_object_mut().unwrap().remove("display_mode").unwrap();
                    row["mode"] = mode;
                    row.as_object_mut().unwrap().remove("canonical_pane_ids");
                } else if platform != "linux" {
                    return Err("Unsupported native platform".into());
                }
                tabs.push(row);
            }
            Ok(
                json!({"native":{"id":copy.workspace.id,"name":copy.workspace.name,"revision":1,"selected_tab_id":copy.workspace.selected_tab_id,
                "selected_pane_id":null,"structure_fingerprint":"","managed_terminal_font_size_points":copy.workspace.managed_terminal_font_size_points,
                "tabs":tabs,"portable_hosts":copy.hosts.hosts,"portable_source_workspace_id":copy.source_workspace_id},"preview":preview}),
            )
        }
        Request::ReferenceView { reference } => {
            reference.validate().map_err(|e| e.to_string())?;
            Ok(match &reference {
                Content::IdeaTree { paths, view_id, .. } => {
                    json!({"title":"Idea Tree source unavailable","message":"These files belong to the source computer. Configure access to those sources before reopening this view.","details":if paths.is_empty(){vec![format!("Original view: {view_id}")]}else{paths.clone()}})
                }
                Content::Lgp {
                    node_id, thread, ..
                } => {
                    json!({"title":"Conversation connection required","message":"Configure a Polar connection to the original node. The conversation reference has been retained.","details":[node_id,thread]})
                }
                Content::ToolSetup { setup_id, .. } => {
                    json!({"title":"Previous setup attempt","message":"This is a saved reference. No setup command will be run on import.","details":[setup_id]})
                }
                _ => {
                    json!({"title":"Content source unavailable","message":"The saved source is retained. Configure the original Host or content source to continue.","details":[]})
                }
            })
        }
    }
}



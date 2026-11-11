//! Data-only business views over the existing local-preview read model.
//! Supplied resolved edges retain reader provenance. No admission, file I/O,
//! camera/geometry, source mutation, or text duplication per perspective.
use serde::Serialize;
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet, VecDeque};

#[derive(Clone, Serialize)]
struct Parent {
    from: String,
    edge: usize,
    reverse: bool,
    slot: String,
}
#[derive(Clone)]
struct Organization {
    root: String,
    nodes: Vec<String>,
    parents: BTreeMap<String, Parent>,
    membership: BTreeMap<String, String>,
    group_parents: BTreeMap<String, Option<String>>,
    group_depths: BTreeMap<String, usize>,
    group_order: Vec<String>,
    max_level: usize,
}

type AdjacentEntry = (usize, String, String, usize, bool);

pub struct PerspectiveSession {
    config: Value,
    graph: Value,
    uuids: BTreeMap<String, Vec<String>>,
    organizations: BTreeMap<String, Organization>,
    projections: BTreeMap<(String, usize), Value>,
    organization_builds: usize,
    partition_builds: usize,
}
fn config_target<'a>(config: &'a Value, owner: &Value, slot: &str) -> Result<&'a Value, String> {
    let key = owner["relations"][slot]["target"]["node"]
        .as_str()
        .ok_or_else(|| format!("Missing local configuration binding: {slot}"))?;
    config["nodes"]
        .get(key)
        .ok_or_else(|| format!("Unknown configuration object: {key}"))
}

fn filter_clauses(config: &Value, filter: &str) -> Result<Vec<Value>, String> {
    if filter == "filter-clear" && config["nodes"].get(filter).is_none() {
        return Ok(Vec::new());
    }
    let clauses = config["nodes"][filter]["properties"]["clauses"]
        .as_array()
        .ok_or("Unknown structured filter")?;
    for clause in clauses {
        if clause["field"] != "node.type"
            || clause["operator"] != "one_of"
            || !clause["values"]
                .as_array()
                .is_some_and(|v| v.iter().all(Value::is_string))
        {
            return Err("Unsupported filter property; no guessed state mapping".into());
        }
    }
    Ok(clauses.clone())
}

/// Validate the capabilities actually implemented by this interpreter, before
/// replacing a global selection. Root availability remains graph-local.
pub fn validate_configuration(text: &str) -> Result<Value, String> {
    let config: Value =
        serde_json::from_str(text).map_err(|e| format!("Invalid configuration JSON: {e}"))?;
    if config["schema"] != "polar.ideacave.itree.v1" {
        return Err("Expected an Idea Tree browsing configuration.".into());
    }
    let root = config["root"]
        .as_str()
        .ok_or("Missing configuration root")?;
    let nodes = config["nodes"]
        .as_object()
        .ok_or("Missing configuration nodes")?;
    if !nodes.get(root).is_some_and(|n| {
        n["properties"]["title"]
            .as_str()
            .is_some_and(|s| !s.trim().is_empty())
    }) {
        return Err("Missing configuration title".into());
    }
    let mut count = 0;
    for view in nodes
        .values()
        .filter(|n| n["properties"]["role"] == "business-perspective")
    {
        count += 1;
        if view["properties"]["title"]
            .as_str()
            .is_none_or(|s| s.trim().is_empty())
        {
            return Err("Missing perspective title".into());
        }
        let roots = config_target(&config, view, "root")?;
        let target = &roots["relations"]["root-00"]["target"];
        if !matches!(target["kind"].as_str(), Some("object" | "tree"))
            || target["uuid"].as_str().is_none()
        {
            return Err("Perspective must reference an existing root UUID".into());
        }
        if target["kind"] == "tree"
            && (target["content"]["id"].as_str().is_none() || target["node"].as_str().is_none())
        {
            return Err("Missing exact root content identity".into());
        }
        let rule = config_target(&config, view, "organization")?;
        if rule["properties"]["algorithm"] != "breadth_first" {
            return Err("Unsupported business organization algorithm".into());
        }
        let policies = rule["properties"]["relation_policy"]
            .as_array()
            .ok_or("Missing relation policy")?;
        for policy in policies {
            if !matches!(
                policy["direction"].as_str(),
                Some("incoming" | "outgoing" | "both")
            ) || (policy["composition"] != true && policy["relation"].as_str().is_none())
            {
                return Err("Invalid relation policy".into());
            }
        }
        let grouping = config_target(&config, view, "grouping")?;
        if grouping["properties"]["algorithm"] != "semantic_anchors"
            || !grouping["properties"]["anchor_node_types"]
                .as_array()
                .is_some_and(|v| v.iter().all(Value::is_string))
        {
            return Err("Unsupported business grouping rule".into());
        }
        if view["relations"].get("filter").is_some() {
            let filter = view["relations"]["filter"]["target"]["node"]
                .as_str()
                .ok_or("Missing default filter binding")?;
            filter_clauses(&config, filter)?;
        }
    }
    if count == 0 {
        return Err(
            "This is a content tree, not a browsing configuration: no perspectives are defined."
                .into(),
        );
    }
    Ok(config)
}
impl PerspectiveSession {
    pub fn new(config: &str, graph: &str) -> Result<Self, String> {
        let config = validate_configuration(config)?;
        let graph: Value = serde_json::from_str(graph).map_err(|e| e.to_string())?;
        let records = graph["records"]
            .as_object()
            .ok_or("Missing reader records")?;
        graph["relations"]
            .as_array()
            .ok_or("Missing reader relations")?;
        let mut uuids: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for (id, record) in records {
            if let Some(uuid) = record["uuid"].as_str().filter(|u| !u.is_empty()) {
                uuids.entry(uuid.to_owned()).or_default().push(id.clone());
            }
        }
        let mut session = Self {
            config,
            graph,
            uuids,
            organizations: BTreeMap::new(),
            projections: BTreeMap::new(),
            organization_builds: 0,
            partition_builds: 0,
        };
        session.add_recorded_roots();
        Ok(session)
    }
    fn resolve_root(&self, view: &Value) -> Result<String, String> {
        let roots = config_target(&self.config, view, "root")?;
        let target = &roots["relations"]["root-00"]["target"];
        let uuid = target["uuid"].as_str().ok_or("Missing root UUID")?;
        if target["kind"] == "tree" && target["content"]["id"].as_str().is_none() {
            return Err("Missing exact root content identity".into());
        }
        let candidates = self
            .uuids
            .get(uuid)
            .ok_or("Perspective root is not loaded")?;
        let matches: Vec<_> = candidates
            .iter()
            .filter(|id| {
                if target["kind"] != "tree" {
                    return true;
                }
                let record = &self.graph["records"][*id];
                record["key"] == target["node"]
                    && record["source_sha256"].as_str()
                        == target["content"]["id"]
                            .as_str()
                            .map(|v| v.trim_start_matches("sha256:"))
            })
            .collect();
        if matches.len() != 1 {
            return Err("Perspective root has ambiguous source versions".into());
        }
        Ok(matches[0].to_string())
    }
    fn add_recorded_roots(&mut self) {
        let has_business =
            self.config["nodes"]
                .as_object()
                .into_iter()
                .flatten()
                .any(|(_, view)| {
                    view["properties"]["role"] == "business-perspective"
                        && self.resolve_root(view).is_ok()
                });
        if has_business {
            return;
        }
        let roots = self.graph["roots"].as_array().cloned().unwrap_or_default();
        for (order, id) in roots.iter().filter_map(Value::as_str).enumerate() {
            let record = &self.graph["records"][id];
            let Some(uuid) = record["uuid"].as_str().filter(|uuid| !uuid.is_empty()) else {
                continue;
            };
            let name = format!("recorded-{uuid}-{order}");
            let root_name = format!("{name}-root");
            let target = if let Some(sha) = record["source_sha256"]
                .as_str()
                .filter(|sha| !sha.is_empty())
            {
                json!({"kind":"tree","uuid":uuid,"node":record["key"],"content":{"id":format!("sha256:{sha}")}})
            } else {
                json!({"kind":"object","uuid":uuid})
            };
            self.config["nodes"][&root_name] = json!({"relations":{"root-00":{"target":target}}});
            self.config["nodes"][&name] = json!({"properties":{"role":"business-perspective","title":record["title"],"business_question":"Browse the recorded structure of this loaded document","data_support":"recorded-structure","order":100+order},"relations":{"root":{"target":{"node":root_name}},"organization":{"target":{"node":"recorded-organization"}},"grouping":{"target":{"node":"grouping-business"}}}});
        }
        self.config["nodes"]["recorded-organization"] = json!({"properties":{"algorithm":"breadth_first","relation_policy":[{"composition":true,"direction":"outgoing"}]}});
    }
    pub fn perspectives(&self) -> Value {
        let mut views: Vec<Value> = self.config["nodes"].as_object().into_iter().flatten()
            .filter(|(_, n)| n["properties"]["role"] == "business-perspective")
            .map(|(id, n)| {
                let root=self.resolve_root(n);
                json!({"id":id,"title":n["properties"]["title"],"question":n["properties"]["business_question"],"order":n["properties"]["order"],"data_support":n["properties"]["data_support"],"default_filter":n["relations"]["filter"]["target"]["node"].as_str().unwrap_or("filter-clear"),"available":root.is_ok(),"root":root.as_ref().ok(),"unavailable_reason":root.err()})
            }).collect();
        views.sort_by_key(|v| v["order"].as_u64().unwrap_or(0));
        json!(views)
    }
    pub fn stats(&self) -> Value {
        json!({"organization_builds":self.organization_builds,"partition_builds":self.partition_builds,"source_nodes":self.graph["records"].as_object().map_or(0, |r|r.len()),"source_relations":self.graph["relations"].as_array().map_or(0, |r|r.len())})
    }
    fn organize(&self, perspective: &str) -> Result<Organization, String> {
        let view = &self.config["nodes"][perspective];
        if view["properties"]["role"] != "business-perspective" {
            return Err("Select a business perspective".into());
        }
        let root = self.resolve_root(view)?;
        let rule = config_target(&self.config, view, "organization")?;
        if rule["properties"]["algorithm"] != "breadth_first" {
            return Err("Unsupported business organization algorithm".into());
        }
        let policies = rule["properties"]["relation_policy"]
            .as_array()
            .ok_or("Missing relation policy")?;
        let group_rule = config_target(&self.config, view, "grouping")?;
        if group_rule["properties"]["algorithm"] != "semantic_anchors" {
            return Err("Unsupported business grouping rule".into());
        }
        let anchor_types = group_rule["properties"]["anchor_node_types"]
            .as_array()
            .ok_or("Missing group types")?;
        let records = self.graph["records"].as_object().ok_or("Missing records")?;
        let edges = self.graph["relations"]
            .as_array()
            .ok_or("Missing relations")?;
        let mut adjacent: BTreeMap<String, Vec<AdjacentEntry>> = BTreeMap::new();
        for (edge_index, edge) in edges.iter().enumerate() {
            if edge["resolved"] != true {
                continue;
            }
            let (Some(from), Some(to)) = (edge["from"].as_str(), edge["to"].as_str()) else {
                continue;
            };
            if !records.contains_key(from) || !records.contains_key(to) {
                continue;
            }
            for (rank, policy) in policies.iter().enumerate() {
                if !(policy["composition"] == true && edge["composition"] == true)
                    && (policy["composition"] == true || policy["relation"] != edge["type"])
                {
                    continue;
                }
                let direction = policy["direction"].as_str().unwrap_or("");
                let slot = edge["slot"].as_str().unwrap_or("").to_owned();
                if matches!(direction, "outgoing" | "both") {
                    adjacent.entry(from.to_owned()).or_default().push((
                        rank,
                        slot.clone(),
                        to.to_owned(),
                        edge_index,
                        false,
                    ));
                }
                if matches!(direction, "incoming" | "both") {
                    adjacent.entry(to.to_owned()).or_default().push((
                        rank,
                        slot,
                        from.to_owned(),
                        edge_index,
                        true,
                    ));
                }
            }
        }
        for list in adjacent.values_mut() {
            list.sort();
        }
        let mut nodes = Vec::new();
        let mut parents = BTreeMap::new();
        let mut children: BTreeMap<String, Vec<String>> = BTreeMap::new();
        let mut seen = BTreeSet::from([root.clone()]);
        let mut queue = VecDeque::from([root.clone()]);
        while let Some(id) = queue.pop_front() {
            nodes.push(id.clone());
            for (_, _, to, edge_index, reverse) in adjacent.get(&id).into_iter().flatten() {
                if !seen.insert(to.clone()) {
                    continue;
                }
                parents.insert(
                    to.clone(),
                    Parent {
                        from: id.clone(),
                        edge: *edge_index,
                        reverse: *reverse,
                        slot: format!("edge_{edge_index}_{}", usize::from(*reverse)),
                    },
                );
                children.entry(id.clone()).or_default().push(to.clone());
                queue.push_back(to.clone());
            }
        }
        let mut membership: BTreeMap<String, String> = BTreeMap::new();
        let mut group_parents = BTreeMap::new();
        let mut group_depths = BTreeMap::new();
        for id in &nodes {
            let parent_group = parents.get(id).map(|p| membership[&p.from].clone());
            if id == &root || anchor_types.contains(&records[id]["type"]) {
                let depth = parent_group.as_ref().map_or(0, |p| group_depths[p] + 1);
                membership.insert(id.clone(), id.clone());
                group_depths.insert(id.clone(), depth);
                group_parents.insert(id.clone(), parent_group);
            } else {
                membership.insert(id.clone(), parent_group.ok_or("Missing parent group")?);
            }
        }
        let mut group_order = Vec::new();
        let mut stack = vec![root.clone()];
        while let Some(id) = stack.pop() {
            if group_parents.contains_key(&id) {
                group_order.push(id.clone());
            }
            if let Some(list) = children.get(&id) {
                stack.extend(list.iter().rev().cloned());
            }
        }
        let max_level = group_depths.values().copied().max().unwrap_or(0);
        Ok(Organization {
            root,
            nodes,
            parents,
            membership,
            group_parents,
            group_depths,
            group_order,
            max_level,
        })
    }
    pub fn projection(
        &mut self,
        perspective: &str,
        requested_level: usize,
    ) -> Result<Value, String> {
        if !self.organizations.contains_key(perspective) {
            let organization = self.organize(perspective)?;
            self.organizations
                .insert(perspective.to_owned(), organization);
            self.organization_builds += 1;
        }
        let organization = &self.organizations[perspective];
        let level = requested_level.min(organization.max_level);
        let key = (perspective.to_owned(), level);
        if let Some(cached) = self.projections.get(&key) {
            return Ok(cached.clone());
        }
        let mut groups: BTreeMap<String, Vec<String>> = BTreeMap::new();
        let mut membership = BTreeMap::new();
        for id in &organization.nodes {
            let mut group = organization.membership[id].clone();
            while organization.group_depths[&group] > level {
                group = organization.group_parents[&group]
                    .clone()
                    .ok_or("Missing coarse group")?;
            }
            groups.entry(group.clone()).or_default().push(id.clone());
            membership.insert(id.clone(), group);
        }
        let groups: Vec<Value> = organization.group_order.iter().filter_map(|id| {
            groups.get(id).map(|members| json!({"id":id,"members":members,"parent":organization.group_parents[id],"title":self.graph["records"][id]["title"],"depth":organization.group_depths[id]}))
        }).collect();
        // Occurrences are presentation data, never admitted graph nodes. Keep
        // canonical membership and every original edge/direction unchanged.
        let mut references = BTreeMap::new();
        let mut reference_edges = Vec::new();
        for (edge_index, edge) in self.graph["relations"]
            .as_array()
            .unwrap()
            .iter()
            .enumerate()
        {
            if edge["resolved"] == false {
                continue;
            }
            let (Some(from), Some(to)) = (edge["from"].as_str(), edge["to"].as_str()) else {
                continue;
            };
            let (Some(a), Some(b)) = (membership.get(from), membership.get(to)) else {
                continue;
            };
            if a == b {
                continue;
            }
            for (group, remote, target_group, local_from, local_to) in
                [(a, to, b, Some(from), None), (b, from, a, None, Some(to))]
            {
                let id = format!(
                    "@group-reference:{}",
                    serde_json::to_string(&(group, remote))
                        .expect("reference identity is serializable")
                );
                references.entry(id.clone()).or_insert_with(
                    || json!({"id":id,"node":remote,"group":group,"target_group":target_group}),
                );
                reference_edges.push(json!({"group":group,"from":local_from.unwrap_or(&id),"to":local_to.unwrap_or(&id),"edge":edge_index}));
            }
        }
        let result = json!({"ok":true,"perspective":perspective,"root":organization.root,"nodes":organization.nodes,"parents":organization.parents,"groups":groups,"membership":membership,"references":references.into_values().collect::<Vec<_>>(),"reference_edges":reference_edges,"level":level,"max_level":organization.max_level});
        self.projections.insert(key, result.clone());
        self.partition_builds += 1;
        Ok(result)
    }
    pub fn matches(&self, filter: &str) -> Result<Value, String> {
        let clauses = filter_clauses(&self.config, filter)?;
        let ids: Vec<&String> = self.graph["records"]
            .as_object()
            .ok_or("Missing records")?
            .iter()
            .filter(|(_, node)| {
                clauses.iter().all(|clause| {
                    clause["values"]
                        .as_array()
                        .is_some_and(|values| values.contains(&node["type"]))
                })
            })
            .map(|(id, _)| id)
            .collect();
        Ok(json!(ids))
    }
    /// Search is always independent of filter and selected perspective.
    pub fn search(&self, query: &str) -> Result<Value, String> {
        let query = query.trim().to_lowercase();
        if query.is_empty() {
            return Ok(json!([]));
        }
        let mut results = Vec::new();
        for (id, node) in self.graph["records"].as_object().ok_or("Missing records")? {
            let title = node["title"].as_str().unwrap_or(id);
            let uuid = node["uuid"].as_str().unwrap_or("");
            let title_lower = title.to_lowercase();
            let exact = title_lower == query || uuid.to_lowercase() == query;
            let hit = exact
                || title_lower.contains(&query)
                || uuid.to_lowercase().contains(&query)
                || node["properties"]
                    .to_string()
                    .to_lowercase()
                    .contains(&query);
            if hit {
                results.push((!exact, id, json!({"id":id,"title":title,"source":node["source"],"key":node["key"],"uuid":uuid})));
            }
        }
        results.sort_by(|a, b| (&a.0, &a.1).cmp(&(&b.0, &b.1)));
        Ok(Value::Array(
            results.into_iter().map(|(_, _, v)| v).collect(),
        ))
    }
}

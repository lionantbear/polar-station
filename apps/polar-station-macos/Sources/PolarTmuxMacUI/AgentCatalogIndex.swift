import Foundation
import PolarTmuxHostClient

struct AgentCatalogIndex {
    var groups: [String] = []
    var roots: [String: [AgentSessionCatalogRowV1]] = [:]
    var children: [String: [AgentSessionCatalogRowV1]] = [:]
    var ranks: [String: Int] = [:]
    var counts: [String: Int] = [:]
    var visible: Set<String> = []
    var terminals: Set<SessionRowProjectionV1.Identity> = []

    init(agents: [AgentSessionCatalogRowV1], groups: [SessionTargetGroupV1], layout: NativeWorkspaceLayoutView?, query: String, filter: String) {
        var paneRanks: [SessionRowProjectionV1.Identity: Int] = [:]
        if let layout {
            let tabs = layout.tabs.filter { $0.tabID == layout.selectedTabID } + layout.tabs.filter { $0.tabID != layout.selectedTabID }
            var rank = 0
            for tab in tabs { for pane in tab.panes {
                if let target = pane.executionTargetID, let session = pane.tmuxSessionID {
                    let identity = SessionRowProjectionV1.Identity(executionTargetID: target, tmuxSessionID: session)
                    paneRanks[identity] = min(paneRanks[identity, default: Int.max], rank)
                }
                rank += 1
            } }
        }
        let projected = projectAgentCatalog(agents: agents, groups: groups,
            ranks: agents.map { ($0.terminals ?? []).compactMap { paneRanks[$0] }.min() }, query: query, filter: filter)
        guard projected.count == agents.count else { return }
        for (i, row) in projected.enumerated() {
            let agent = agents[i]
            terminals.formUnion(agent.terminals ?? [])
            ranks[agent.id] = row.rank ?? Int.max
            if row.matches { visible.insert(agent.id) }
            if let parent = row.parent { children[agents[parent].id, default: []].append(agent) }
            if row.parent == nil && row.matches { roots[row.group, default: []].append(agent) }
        }
        for row in projected where roots[row.group] != nil { counts[row.group, default: 0] += 1 }
        self.groups = roots.keys.sorted()
    }
}

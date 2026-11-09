import Foundation

struct NativeSurfaceRetirementState: Equatable, Sendable {
    enum Phase: Equatable, Sendable {
        case active
        case detachedFromTree
        case retiringContent
        case awaitingGhosttyClose
        case closeObserved
        case releasable
        case retirementFailed
    }

    private struct Record: Equatable, Sendable {
        let paneID: String
        let surfaceToken: String
        var detachedFromTree = false
        var contentRetirementStarted = false
        var contentDetached = false
        var closeObserved = false
        var retirementFailed = false

        var phase: Phase {
            if retirementFailed { return .retirementFailed }
            if contentDetached && closeObserved { return .releasable }
            if closeObserved { return .closeObserved }
            if contentDetached { return .awaitingGhosttyClose }
            if contentRetirementStarted { return .retiringContent }
            if detachedFromTree { return .detachedFromTree }
            return .active
        }

        var canRelease: Bool {
            contentDetached && closeObserved && !retirementFailed
        }
    }

    private var records: [String: Record] = [:]

    mutating func register(
        leaseID: String,
        paneID: String,
        surfaceToken: String
    ) {
        precondition(records[leaseID] == nil)
        records[leaseID] = Record(
            paneID: paneID,
            surfaceToken: surfaceToken
        )
    }

    mutating func detachFromTree(leaseID: String) {
        guard var record = records[leaseID] else { return }
        record.detachedFromTree = true
        records[leaseID] = record
    }

    mutating func beginContentRetirement(leaseID: String) {
        guard var record = records[leaseID], record.detachedFromTree else {
            return
        }
        record.contentRetirementStarted = true
        records[leaseID] = record
    }

    @discardableResult
    mutating func markContentDetached(leaseID: String) -> Bool {
        guard var record = records[leaseID],
              record.contentRetirementStarted,
              !record.retirementFailed
        else { return false }
        record.contentDetached = true
        records[leaseID] = record
        return record.canRelease
    }

    mutating func markRetirementFailed(leaseID: String) {
        guard var record = records[leaseID] else { return }
        record.retirementFailed = true
        records[leaseID] = record
    }

    @discardableResult
    mutating func retryContentRetirement(leaseID: String) -> Bool {
        guard var record = records[leaseID], record.retirementFailed else {
            return false
        }
        record.retirementFailed = false
        record.contentRetirementStarted = true
        records[leaseID] = record
        return true
    }

    mutating func observeClose(
        surfaceToken: String,
        processAlive: Bool
    ) -> [String] {
        guard !processAlive else { return [] }
        var releasable: [String] = []
        for leaseID in records.keys.sorted() {
            guard var record = records[leaseID],
                  record.surfaceToken == surfaceToken,
                  !record.closeObserved
            else { continue }
            record.closeObserved = true
            records[leaseID] = record
            if record.canRelease {
                releasable.append(leaseID)
            }
        }
        return releasable
    }

    @discardableResult
    mutating func release(leaseID: String) -> Bool {
        guard let record = records[leaseID], record.canRelease else {
            return false
        }
        records.removeValue(forKey: leaseID)
        return true
    }

    func phase(for leaseID: String) -> Phase? {
        records[leaseID]?.phase
    }
}

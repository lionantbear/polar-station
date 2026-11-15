import Foundation

/// Presentation geometry owned entirely by the macOS adapter.
///
/// Rust never supplies these columns, rows, ratios, roles, or trees. The
/// native Workspace model calls this family after each immediate UI intent and
/// the native Ghostty executor realizes the returned fixed topology.
enum PolarStationNativeDisplayMode: String, Codable, Equatable, Sendable {
    case monitor
    case focus
}

struct PolarStationNativeRatio: Equatable, Hashable, Sendable {
    let numerator: Int
    let denominator: Int

    init(_ numerator: Int, _ denominator: Int) {
        precondition(numerator > 0 && denominator > 0)
        self.numerator = numerator
        self.denominator = denominator
    }
}

enum PolarStationNativeLayoutRole: Equatable, Hashable, Sendable {
    case monitor
    case main
    case sidebar(slot: Int)
}

struct PolarStationNativeLayoutSlot: Equatable, Sendable {
    let surfaceID: String
    let visualIndex: Int
    let column: Int
    let row: Int
    let width: PolarStationNativeRatio
    let height: PolarStationNativeRatio
    let role: PolarStationNativeLayoutRole
}

struct PolarStationNativeLayoutColumn: Equatable, Sendable {
    let index: Int
    let width: PolarStationNativeRatio
    let surfaceIDs: [String]
}

indirect enum PolarStationNativeLayoutTree: Equatable, Sendable {
    enum Axis: Equatable, Sendable {
        case horizontal
        case vertical
    }

    case surface(String)
    case split(Axis, PolarStationNativeLayoutTree, PolarStationNativeLayoutTree)
}

extension PolarStationNativeLayoutTree: CustomStringConvertible {
    var description: String {
        switch self {
        case .surface(let identifier):
            return identifier
        case .split(let axis, let first, let second):
            let prefix = axis == .horizontal ? "H" : "V"
            return "\(prefix)(\(first),\(second))"
        }
    }
}

struct PolarStationNativeLayout: Equatable, Sendable {
    let mode: PolarStationNativeDisplayMode
    let visualPaneIDs: [String]
    let columnPopulations: [Int]
    let columns: [PolarStationNativeLayoutColumn]
    let slots: [PolarStationNativeLayoutSlot]
    let tree: PolarStationNativeLayoutTree
}

enum PolarStationNativeLayoutError: Error, Equatable {
    case unsupportedPaneCount
    case invalidPaneIdentity
    case missingMainPane
}

enum PolarStationNativeLayoutFamily {
    private static let monitorPopulations = [
        [1], [1, 1], [1, 2], [1, 2, 1], [1, 2, 2],
        [2, 2, 2], [2, 3, 2], [2, 3, 3], [3, 3, 3],
    ]

    private static let focusPopulations = [
        [1], [1, 1], [1, 2], [1, 2, 1], [1, 2, 2],
        [1, 2, 3], [1, 3, 3], [1, 3, 4], [1, 4, 4],
    ]

    static func layout(
        orderedPaneIDs: [String],
        mode: PolarStationNativeDisplayMode,
        mainPaneID: String?
    ) throws -> PolarStationNativeLayout {
        guard (1...9).contains(orderedPaneIDs.count) else {
            throw PolarStationNativeLayoutError.unsupportedPaneCount
        }
        guard Set(orderedPaneIDs).count == orderedPaneIDs.count,
              orderedPaneIDs.allSatisfy(validIdentifier)
        else {
            throw PolarStationNativeLayoutError.invalidPaneIdentity
        }

        let visualPaneIDs: [String]
        let populations: [Int]
        switch mode {
        case .monitor:
            visualPaneIDs = orderedPaneIDs
            populations = monitorPopulations[orderedPaneIDs.count - 1]
        case .focus:
            guard let mainPaneID, orderedPaneIDs.contains(mainPaneID) else {
                throw PolarStationNativeLayoutError.missingMainPane
            }
            visualPaneIDs = [mainPaneID] + orderedPaneIDs.filter { $0 != mainPaneID }
            populations = focusPopulations[orderedPaneIDs.count - 1]
        }

        let columnWidths = widths(mode: mode, paneCount: orderedPaneIDs.count)
        var paneIterator = visualPaneIDs.makeIterator()
        var visualIndex = 0
        var slots: [PolarStationNativeLayoutSlot] = []
        var columns: [PolarStationNativeLayoutColumn] = []
        var columnTrees: [PolarStationNativeLayoutTree] = []

        for (columnIndex, population) in populations.enumerated() {
            var surfaceIDs: [String] = []
            var rowTrees: [PolarStationNativeLayoutTree] = []
            for row in 0..<population {
                guard let surfaceID = paneIterator.next() else {
                    throw PolarStationNativeLayoutError.unsupportedPaneCount
                }
                let role: PolarStationNativeLayoutRole
                switch mode {
                case .monitor:
                    role = .monitor
                case .focus where visualIndex == 0:
                    role = .main
                case .focus:
                    role = .sidebar(slot: visualIndex)
                }
                slots.append(.init(
                    surfaceID: surfaceID,
                    visualIndex: visualIndex,
                    column: columnIndex,
                    row: row,
                    width: columnWidths[columnIndex],
                    height: .init(1, population),
                    role: role
                ))
                surfaceIDs.append(surfaceID)
                rowTrees.append(.surface(surfaceID))
                visualIndex += 1
            }
            columns.append(.init(
                index: columnIndex,
                width: columnWidths[columnIndex],
                surfaceIDs: surfaceIDs
            ))
            columnTrees.append(rightAssociated(rowTrees, axis: .vertical))
        }

        return .init(
            mode: mode,
            visualPaneIDs: visualPaneIDs,
            columnPopulations: populations,
            columns: columns,
            slots: slots,
            tree: rightAssociated(columnTrees, axis: .horizontal)
        )
    }

    private static func widths(
        mode: PolarStationNativeDisplayMode,
        paneCount: Int
    ) -> [PolarStationNativeRatio] {
        switch mode {
        case .monitor:
            let columnCount = monitorPopulations[paneCount - 1].count
            return Array(repeating: .init(1, columnCount), count: columnCount)
        case .focus where paneCount == 1:
            return [.init(1, 1)]
        case .focus where paneCount <= 3:
            return [.init(4, 5), .init(1, 5)]
        case .focus:
            return [.init(3, 5), .init(1, 5), .init(1, 5)]
        }
    }

    private static func rightAssociated(
        _ nodes: [PolarStationNativeLayoutTree],
        axis: PolarStationNativeLayoutTree.Axis
    ) -> PolarStationNativeLayoutTree {
        precondition(!nodes.isEmpty)
        guard nodes.count > 1 else { return nodes[0] }
        return .split(
            axis,
            nodes[0],
            rightAssociated(Array(nodes.dropFirst()), axis: axis)
        )
    }

    private static func validIdentifier(_ value: String) -> Bool {
        !value.isEmpty
            && value.utf8.count <= 256
            && value == value.trimmingCharacters(in: .whitespacesAndNewlines)
            && value.unicodeScalars.allSatisfy {
                !CharacterSet.controlCharacters.contains($0)
            }
    }
}

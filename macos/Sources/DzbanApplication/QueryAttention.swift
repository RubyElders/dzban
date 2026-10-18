import Foundation

enum QueryCompletionOutcome: Equatable {
    case success
    case failure
}

struct QueryAttentionUpdate {
    let unreadCount: Int
    let requestAttention: Bool
    let cancelAttention: Bool
}

struct QueryAttentionState {
    private(set) var applicationVisible = true
    private(set) var unseen: [UUID: QueryCompletionOutcome] = [:]
    private var requestedWhileAway = false

    mutating func complete(session: UUID, outcome: QueryCompletionOutcome, selected: Bool) -> Bool {
        if applicationVisible && selected {
            unseen.removeValue(forKey: session)
            return false
        }
        unseen[session] = outcome
        guard !applicationVisible, !requestedWhileAway else { return false }
        requestedWhileAway = true
        return true
    }

    mutating func setVisible(_ visible: Bool, selected: UUID?) {
        applicationVisible = visible
        if visible {
            requestedWhileAway = false
            if let selected { unseen.removeValue(forKey: selected) }
        }
    }

    mutating func viewed(_ session: UUID) {
        if applicationVisible { unseen.removeValue(forKey: session) }
    }

    mutating func remove(_ session: UUID) { unseen.removeValue(forKey: session) }
    mutating func clear() { unseen.removeAll(); requestedWhileAway = false }
}

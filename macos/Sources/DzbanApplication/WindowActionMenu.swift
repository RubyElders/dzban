import SwiftUI

struct WindowActionMenu: View {
    @ObservedObject var model: WindowActionsModel
    let restoreFocus: () -> Void

    var body: some View {
        ScrollViewReader { proxy in
            ScrollView {
                LazyVStack(alignment: .leading, spacing: 2) {
                    ForEach(model.items) { action in
                        Button { model.dismiss(); restoreFocus(); action.perform() } label: {
                            VStack(alignment: .leading, spacing: 3) {
                                Text(action.title)
                                Text(action.detail).font(.caption).foregroundStyle(.secondary)
                            }.frame(maxWidth: .infinity, alignment: .leading).padding(7)
                            .contentShape(Rectangle())
                            .background(model.highlighted == action.id ? Color.accentColor.opacity(0.15) : .clear, in: RoundedRectangle(cornerRadius: 5))
                        }
                        .buttonStyle(.plain)
                        .disabled(!action.enabled)
                        .accessibilityIdentifier("action.\(action.id)")
                        .accessibilityAddTraits(model.highlighted == action.id ? .isSelected : [])
                        .id(action.id)
                    }
                    if model.items.isEmpty { Text("No matching actions").foregroundStyle(.secondary).padding(7).accessibilityIdentifier("actions.empty") }
                }.padding(8)
            }
            .onChange(of: model.highlighted) { _, id in
                if let id { proxy.scrollTo(id) }
            }
        }
    }

}

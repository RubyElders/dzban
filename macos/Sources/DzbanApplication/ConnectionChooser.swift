import AppKit
import DzbanBridge
import SwiftUI

struct ConnectionChooser: View {
    @ObservedObject var store: ApplicationStore
    var focusedProfile: FocusState<String?>.Binding
    @Binding var selectedProfile: String?
    let focusConnection: () -> Void
    let moveConnection: (Int) -> Void

    private func backend(_ profile: String) -> String {
        store.profiles.first { $0.name == profile }?.backend ?? "postgresql"
    }

    var body: some View {
        ScrollViewReader { proxy in
            ScrollView {
                VStack(spacing: 6) {
                    HStack {
                        Text("Connections").font(.headline)
                        Spacer()
                        Button("Test All") { Task { await store.probeAllProfiles() } }
                            .disabled(!store.navigationAvailable).accessibilityIdentifier("connections.probe-all")
                    }.padding(.bottom, 4)
                    if store.profiles.isEmpty {
                        Text("No connection profiles found").foregroundStyle(.secondary).accessibilityIdentifier("connections.empty")
                        Button("Reload Profiles") { Task { await store.loadProfiles() } }
                    } else {
                        ForEach(store.profiles) { profile in
                            HStack(spacing: 12) {
                                Button { selectedProfile = profile.name; Task { await store.connect(profile: profile.name) } } label: {
                                    HStack {
                                        DatabaseArtwork(backend: backend(profile.name))
                                        Text(profile.name).lineLimit(1)
                                        Spacer(minLength: 0)
                                    }
                                    .frame(maxWidth: .infinity, alignment: .leading).frame(height: 34)
                                    .contentShape(Rectangle())
                                }
                                .buttonStyle(.plain)
                                .disabled(store.pendingProfile != nil || !store.navigationAvailable)
                                .focusable(false)
                                .focusEffectDisabled()
                                .accessibilityIdentifier("connection.\(profile.name)")
                                .accessibilityLabel("Connect to \(profile.name)")
                                HStack(spacing: 6) {
                                    ZStack {
                                        if store.profileHealth[profile.name]?.probing == true || store.pendingProfile == profile.name {
                                            ProgressView().controlSize(.mini)
                                        }
                                    }.frame(width: 16, height: 16)
                                    if let health = store.profileHealth[profile.name], !health.probing {
                                        Text(health.failure ?? "\(health.state.label) - \(ConnectionHealth.latency(health.latencyMicros))")
                                            .foregroundStyle(health.state == .error ? Color.red : Color.primary)
                                            .lineLimit(1).help(health.failure ?? health.version ?? health.state.label)
                                            .accessibilityIdentifier("connection.health.\(profile.name)")
                                    }
                                }.font(.callout).frame(width: 180, alignment: .trailing)
                                Button("Open") { selectedProfile = profile.name; Task { await store.connect(profile: profile.name) } }
                                    .disabled(store.pendingProfile != nil || !store.navigationAvailable)
                                    .accessibilityIdentifier("connection.open.\(profile.name)")
                                Button("Test") { selectedProfile = profile.name; Task { await store.probeProfile(profile.name) } }
                                    .disabled(store.profileHealth[profile.name]?.probing == true || !store.navigationAvailable)
                                    .accessibilityIdentifier("connection.probe.\(profile.name)")
                            }.padding(.horizontal, 8).frame(height: 38)
                                .frame(maxWidth: .infinity)
                                .focusable()
                                .focusEffectDisabled()
                                .focused(focusedProfile, equals: profile.name)
                                .onKeyPress(.return) {
                                    guard store.pendingProfile == nil, store.navigationAvailable else { return .ignored }
                                    Task { await store.connect(profile: profile.name) }
                                    return .handled
                                }
                                .onKeyPress(.space) {
                                    guard store.pendingProfile == nil, store.navigationAvailable else { return .handled }
                                    Task { await store.probeProfile(profile.name) }
                                    return .handled
                                }
                                .onKeyPress(.downArrow) { moveConnection(1); return .handled }
                                .onKeyPress(.upArrow) { moveConnection(-1); return .handled }
                                .background(selectedProfile == profile.name ? Color.accentColor.opacity(0.12) : Color.clear, in: RoundedRectangle(cornerRadius: 4))
                                .overlay(RoundedRectangle(cornerRadius: 4).stroke(selectedProfile == profile.name ? Color.accentColor : Color.clear, lineWidth: 1.5))
                                .accessibilityElement(children: .contain)
                                .accessibilityIdentifier("connection.row.\(profile.name)")
                                .accessibilityLabel(profile.name)
                                .accessibilityAddTraits(selectedProfile == profile.name ? .isSelected : [])
                                .id(profile.name)
                            Divider()
                        }
                    }
                }
                .padding(12)
            }
            .onChange(of: focusedProfile.wrappedValue) { _, profile in
                if let profile { selectedProfile = profile; proxy.scrollTo(profile, anchor: .center) }
            }
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .accessibilityElement(children: .contain)
        .accessibilityIdentifier("connections.chooser")
        .onAppear { focusConnection() }
        .onKeyPress(.escape) { Task { @MainActor in store.cancelConnectionPicker() }; return .handled }
    }

}

import SwiftUI

@main
struct MG101StudioApp: App {
    @StateObject private var state = StudioState()

    var body: some Scene {
        WindowGroup {
            ContentView()
                .environmentObject(state)
                .frame(minWidth: 1080, minHeight: 700)
        }
        .commands {
            CommandGroup(replacing: .newItem) {
                Button("Import Patches…") { state.importPatchPanel() }
                    .keyboardShortcut("o")
                Button("Export Selected Patch…") { state.exportCurrentPatch() }
                    .keyboardShortcut("s", modifiers: [.command, .shift])
                    .disabled(state.patch == nil)
                Button("Export Patch Set…") { state.beginSetExport() }
                    .disabled(state.patches.count < 36)
                Divider()
                Button("Import Device Profile…") { state.importProfilePanel() }
                Button("Use Bundled Device Profile") { state.useBundledProfile() }
                    .disabled(state.profileSource == nil)
            }
            CommandGroup(after: .undoRedo) {
                Button("Undo Patch Change") { state.undo() }
                    .keyboardShortcut("z")
                Button("Redo Patch Change") { state.redo() }
                    .keyboardShortcut("z", modifiers: [.command, .shift])
            }
        }
        Settings {
            AISettingsView()
                .environmentObject(state)
        }
    }
}

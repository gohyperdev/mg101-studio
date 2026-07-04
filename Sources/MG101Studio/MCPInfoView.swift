import AppKit
import SwiftUI

struct MCPInfoView: View {
    private var executablePath: String {
        let own = Bundle.main.executableURL
        return own?.deletingLastPathComponent()
            .appendingPathComponent("MG101MCP").path
            ?? ".build/release/MG101MCP"
    }

    private var configuration: String {
        """
        {
          "mcpServers": {
            "mg101-studio": {
              "command": "\(executablePath)"
            }
          }
        }
        """
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 14) {
            Label("MCP server", systemImage: "point.3.connected.trianglepath.dotted")
                .font(.headline)
            Text("External agents launch the bundled stdio server. It exposes model-aware file tools and never overwrites an existing output.")
                .font(.caption)
                .foregroundStyle(.secondary)
            Text(executablePath)
                .font(.caption.monospaced())
                .textSelection(.enabled)
            TextEditor(text: .constant(configuration))
                .font(.caption.monospaced())
                .frame(minHeight: 180)
            Button("Copy MCP configuration") {
                NSPasteboard.general.clearContents()
                NSPasteboard.general.setString(configuration, forType: .string)
            }
            .buttonStyle(.borderedProminent)
            Spacer()
        }
        .padding()
    }
}

import Foundation
import Network
import MG101Core
import MG101Tools

final class MCPBridgeServer: Sendable {
    private let listener: NWListener
    let token: String
    let port: UInt16
    private let executor: GUIToolExecutor

    @MainActor
    init(state: StudioState, preferredPort: UInt16 = 10101) throws {
        self.executor = GUIToolExecutor(state: state)

        // Generate random 32-character hex token
        let characters = "abcdef0123456789"
        self.token = String((0..<32).map { _ in characters.randomElement()! })

        // Save token to ~/.mg101_bridge_token
        let homeDir = FileManager.default.homeDirectoryForCurrentUser
        let tokenURL = homeDir.appendingPathComponent(".mg101_bridge_token")
        try token.write(to: tokenURL, atomically: true, encoding: .utf8)

        // Try listening on preferredPort, fallback to random if occupied
        var selectedPort = preferredPort
        var nwListener: NWListener?

        for p in preferredPort..<(preferredPort + 100) {
            if let l = try? NWListener(using: .tcp, on: NWEndpoint.Port(rawValue: p)!) {
                nwListener = l
                selectedPort = p
                break
            }
        }

        guard let listener = nwListener else {
            throw NSError(domain: "MCPBridgeServer", code: 1, userInfo: [NSLocalizedDescriptionKey: "No free port available"])
        }

        self.listener = listener
        self.port = selectedPort

        // Save port to ~/.mg101_bridge_port
        let portURL = homeDir.appendingPathComponent(".mg101_bridge_port")
        try String(selectedPort).write(to: portURL, atomically: true, encoding: .utf8)
    }

    func start() {
        listener.stateUpdateHandler = { state in
            print("MCPBridge: listener state: \(state)")
        }
        listener.newConnectionHandler = { [weak self] connection in
            self?.handleConnection(connection)
        }
        listener.start(queue: .global(qos: .default))
        print("MCPBridge started on port \(port) with token auth.")
    }

    func stop() {
        listener.cancel()
    }

    private func handleConnection(_ connection: NWConnection) {
        connection.start(queue: .global(qos: .default))
        readRequest(connection: connection, buffer: Data())
    }

    private func readRequest(connection: NWConnection, buffer: Data) {
        connection.receive(minimumIncompleteLength: 1, maximumLength: 65536) { [weak self] content, _, isComplete, error in
            guard let self else { return }
            if error != nil {
                connection.cancel()
                return
            }

            var newBuffer = buffer
            if let content {
                newBuffer.append(content)
            }

            if let range = newBuffer.range(of: Data("\r\n\r\n".utf8)) {
                let headerData = newBuffer.subdata(in: 0..<range.lowerBound)
                let bodyData = newBuffer.subdata(in: range.upperBound..<newBuffer.count)

                let headerStr = String(decoding: headerData, as: UTF8.self)
                let lines = headerStr.components(separatedBy: "\r\n")
                guard !lines.isEmpty else {
                    self.sendResponse(connection: connection, status: 400, body: "Bad Request")
                    return
                }

                let requestLine = lines[0].components(separatedBy: " ")
                guard requestLine.count >= 3 else {
                    self.sendResponse(connection: connection, status: 400, body: "Bad Request")
                    return
                }

                let method = requestLine[0]
                let path = requestLine[1]

                var contentLength = 0
                var authHeader: String?
                for line in lines {
                    let parts = line.split(separator: ":", maxSplits: 1).map { $0.trimmingCharacters(in: .whitespaces) }
                    if parts.count == 2 {
                        if parts[0].lowercased() == "content-length", let len = Int(parts[1]) {
                            contentLength = len
                        }
                        if parts[0].lowercased() == "authorization" {
                            authHeader = parts[1]
                        }
                        if parts[0].lowercased() == "x-api-key" {
                            authHeader = "Bearer \(parts[1])"
                        }
                    }
                }

                let expectedAuth = "Bearer \(self.token)"
                guard authHeader == expectedAuth else {
                    self.sendResponse(connection: connection, status: 401, body: "Unauthorized")
                    return
                }

                if bodyData.count < contentLength {
                    self.readRemainingBody(connection: connection, buffer: newBuffer, expectedLength: range.upperBound + contentLength)
                    return
                }

                let finalBody = bodyData.prefix(contentLength)
                self.processRequest(connection: connection, method: method, path: path, body: finalBody)
            } else if isComplete {
                connection.cancel()
            } else {
                self.readRequest(connection: connection, buffer: newBuffer)
            }
        }
    }

    private func readRemainingBody(connection: NWConnection, buffer: Data, expectedLength: Int) {
        connection.receive(minimumIncompleteLength: 1, maximumLength: 65536) { [weak self] content, _, isComplete, error in
            guard let self else { return }
            if error != nil {
                connection.cancel()
                return
            }
            var newBuffer = buffer
            if let content {
                newBuffer.append(content)
            }
            if newBuffer.count >= expectedLength {
                if let range = newBuffer.range(of: Data("\r\n\r\n".utf8)) {
                    let headerData = newBuffer.subdata(in: 0..<range.lowerBound)
                    let bodyData = newBuffer.subdata(in: range.upperBound..<newBuffer.count)

                    let headerStr = String(decoding: headerData, as: UTF8.self)
                    let lines = headerStr.components(separatedBy: "\r\n")
                    let requestLine = lines[0].components(separatedBy: " ")
                    let method = requestLine[0]
                    let path = requestLine[1]

                    var contentLength = 0
                    for line in lines {
                        let parts = line.split(separator: ":", maxSplits: 1).map { $0.trimmingCharacters(in: .whitespaces) }
                        if parts.count == 2 && parts[0].lowercased() == "content-length" {
                            contentLength = Int(parts[1]) ?? 0
                        }
                    }

                    let finalBody = bodyData.prefix(contentLength)
                    self.processRequest(connection: connection, method: method, path: path, body: finalBody)
                }
            } else if isComplete {
                connection.cancel()
            } else {
                self.readRemainingBody(connection: connection, buffer: newBuffer, expectedLength: expectedLength)
            }
        }
    }

    private func processRequest(connection: NWConnection, method: String, path: String, body: Data) {
        Task {
            do {
                if path == "/tools/list" {
                    let tools = ToolDefinition.allDefinitions(variant: .library).map { toolDef in
                        [
                            "name": toolDef.name,
                            "description": toolDef.description,
                            "inputSchema": toolDef.inputSchema
                        ]
                    }
                    let responseData = try JSONSerialization.data(withJSONObject: tools)
                    self.sendResponse(connection: connection, status: 200, bodyData: responseData)
                } else if path == "/tools/call" {
                    struct ToolCallRequest: Decodable {
                        let name: String
                        let arguments: [String: JSONValue]
                    }

                    let req = try JSONDecoder().decode(ToolCallRequest.self, from: body)
                    let cmd = try DomainCommand.parse(name: req.name, arguments: req.arguments)

                    // Implicitly schedules on MainActor because executor is `@MainActor` isolated
                    let result = try await self.executor.execute(command: cmd)

                    let responseData = try JSONEncoder().encode(result)
                    self.sendResponse(connection: connection, status: 200, bodyData: responseData)
                } else {
                    self.sendResponse(connection: connection, status: 404, body: "Not Found")
                }
            } catch {
                let errorObj = ["error": error.localizedDescription]
                let errorData = (try? JSONSerialization.data(withJSONObject: errorObj)) ?? Data("Error".utf8)
                self.sendResponse(connection: connection, status: 500, bodyData: errorData)
            }
        }
    }

    private func sendResponse(connection: NWConnection, status: Int, body: String) {
        sendResponse(connection: connection, status: status, bodyData: Data(body.utf8))
    }

    private func sendResponse(connection: NWConnection, status: Int, bodyData: Data) {
        let statusString: String
        switch status {
        case 200: statusString = "200 OK"
        case 400: statusString = "400 Bad Request"
        case 401: statusString = "401 Unauthorized"
        case 404: statusString = "404 Not Found"
        default: statusString = "500 Internal Server Error"
        }

        var responseStr = "HTTP/1.1 \(statusString)\r\n"
        responseStr.append("Content-Type: application/json\r\n")
        responseStr.append("Content-Length: \(bodyData.count)\r\n")
        responseStr.append("Connection: close\r\n\r\n")

        var fullResponse = Data(responseStr.utf8)
        fullResponse.append(bodyData)

        connection.send(content: fullResponse, completion: .contentProcessed({ _ in
            connection.cancel()
        }))
    }
}

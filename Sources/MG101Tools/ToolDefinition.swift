import Foundation

public enum ToolKind: String, Sendable, Codable {
    case read
    case write
    case filesystem
    case destructive
}

public enum ToolVariant: Sendable {
    case library
    case file
}

public enum JSONValue: Sendable, Codable, Equatable {
    case null
    case bool(Bool)
    case number(Double)
    case string(String)
    case array([JSONValue])
    case object([String: JSONValue])

    public init(from decoder: Decoder) throws {
        let container = try decoder.singleValueContainer()
        if container.decodeNil() {
            self = .null
        } else if let val = try? container.decode(Bool.self) {
            self = .bool(val)
        } else if let val = try? container.decode(Double.self) {
            self = .number(val)
        } else if let val = try? container.decode(String.self) {
            self = .string(val)
        } else if let val = try? container.decode([JSONValue].self) {
            self = .array(val)
        } else if let val = try? container.decode([String: JSONValue].self) {
            self = .object(val)
        } else {
            throw DecodingError.dataCorruptedError(in: container, debugDescription: "Invalid JSONValue")
        }
    }

    public func encode(to encoder: Encoder) throws {
        var container = encoder.singleValueContainer()
        switch self {
        case .null: try container.encodeNil()
        case .bool(let b): try container.encode(b)
        case .number(let n): try container.encode(n)
        case .string(let s): try container.encode(s)
        case .array(let arr): try container.encode(arr)
        case .object(let obj): try container.encode(obj)
        }
    }
    
    // Quick helpers
    public var stringValue: String? {
        if case .string(let s) = self { return s }
        return nil
    }
    public var intValue: Int? {
        if case .number(let n) = self { return Int(n) }
        return nil
    }
    public var boolValue: Bool? {
        if case .bool(let b) = self { return b }
        return nil
    }
    public var arrayValue: [JSONValue]? {
        if case .array(let a) = self { return a }
        return nil
    }
    public var objectValue: [String: JSONValue]? {
        if case .object(let o) = self { return o }
        return nil
    }
}

public struct ToolDefinition: Sendable, Codable, Equatable {
    public let name: String
    public let description: String
    public let inputSchema: JSONValue
    public let kind: ToolKind

    public init(name: String, description: String, inputSchema: JSONValue, kind: ToolKind) {
        self.name = name
        self.description = description
        self.inputSchema = inputSchema
        self.kind = kind
    }
}

extension ToolDefinition {
    public static func allDefinitions(variant: ToolVariant) -> [ToolDefinition] {
        func schema(properties: [String: JSONValue], required: [String] = []) -> JSONValue {
            .object([
                "type": .string("object"),
                "properties": .object(properties),
                "required": .array(required.map { .string($0) }),
                "additionalProperties": .bool(false)
            ])
        }

        func stringProperty(_ desc: String) -> JSONValue {
            .object(["type": .string("string"), "description": .string(desc)])
        }

        func integerProperty(_ desc: String) -> JSONValue {
            .object(["type": .string("integer"), "description": .string(desc)])
        }

        func boolProperty(_ desc: String) -> JSONValue {
            .object(["type": .string("boolean"), "description": .string(desc)])
        }

        // Setup base targets based on variant
        var targetProps: [String: JSONValue] = [:]
        var targetReq: [String] = []

        switch variant {
        case .library:
            targetProps["patchID"] = stringProperty("ID patcha w bibliotece.")
            targetProps["expectedRevision"] = integerProperty("Oczekiwana rewizja patcha w celu uniknięcia konfliktów współbieżności.")
            targetReq = ["patchID", "expectedRevision"]
        case .file:
            targetProps["input"] = stringProperty("Ścieżka bezwzględna do pliku źródłowego .mg101patch.")
            targetProps["output"] = stringProperty("Ścieżka bezwzględna do docelowego pliku wyjściowego .mg101patch.")
            targetReq = ["input", "output"]
        }

        var tools: [ToolDefinition] = []

        // --- READ TOOLS ---
        tools.append(ToolDefinition(
            name: "list_patches",
            description: "Wypisuje listę patchy w bibliotece (slot, ID, nazwa, origin, rewizja). Działa tylko w trybie bibliotecznym.",
            inputSchema: schema(properties: [:]),
            kind: .read
        ))

        tools.append(ToolDefinition(
            name: "get_patch",
            description: "Pobiera szczegółowe dane patcha, w tym aktywne modele i parametry.",
            inputSchema: schema(properties: ["patchID": stringProperty("ID patcha")], required: ["patchID"]),
            kind: .read
        ))

        tools.append(ToolDefinition(
            name: "get_selection",
            description: "Zwraca aktualnie zaznaczony patch i blok w GUI. Działa tylko w trybie bibliotecznym.",
            inputSchema: schema(properties: [:]),
            kind: .read
        ))

        tools.append(ToolDefinition(
            name: "list_models",
            description: "Wypisuje dostępne modele i parametry dla danego bloku (np. amp, efx).",
            inputSchema: schema(properties: ["block": stringProperty("ID bloku")], required: ["block"]),
            kind: .read
        ))

        tools.append(ToolDefinition(
            name: "get_profile",
            description: "Zwraca pełny profil aktywnego urządzenia (bloki, nazwane pola, ograniczenia).",
            inputSchema: schema(properties: [:]),
            kind: .read
        ))

        tools.append(ToolDefinition(
            name: "get_diff",
            description: "Zwraca różnice bajtowe patcha względem oryginału. Działa tylko w trybie bibliotecznym.",
            inputSchema: schema(properties: ["patchID": stringProperty("ID patcha")], required: ["patchID"]),
            kind: .read
        ))

        // --- WRITE TOOLS ---
        var setParamProps = targetProps
        setParamProps["block"] = stringProperty("ID bloku (np. amp).")
        setParamProps["parameter"] = stringProperty("Nazwa parametru do zmiany.")
        setParamProps["value"] = integerProperty("Nowa wartość parametru.")
        tools.append(ToolDefinition(
            name: "set_parameter",
            description: "Modyfikuje wskazany parametr w aktywnym modelu wybranego bloku.",
            inputSchema: schema(properties: setParamProps, required: targetReq + ["block", "parameter", "value"]),
            kind: .write
        ))

        var setModelProps = targetProps
        setModelProps["block"] = stringProperty("ID bloku (np. amp).")
        setModelProps["model"] = integerProperty("ID modelu dla tego bloku.")
        setModelProps["values"] = .object([
            "type": .string("array"),
            "items": .object(["type": .string("integer")]),
            "description": .string("Wartości parametrów w kolejności zgodnej z katalogiem efektów.")
        ])
        setModelProps["bypassed"] = boolProperty("Czy model ma być od razu pominięty (bypassed).")
        tools.append(ToolDefinition(
            name: "set_model",
            description: "Zmienia aktywny model bloku i ustawia jego parametry.",
            inputSchema: schema(properties: setModelProps, required: targetReq + ["block", "model", "values", "bypassed"]),
            kind: .write
        ))

        var setBypassProps = targetProps
        setBypassProps["block"] = stringProperty("ID bloku.")
        setBypassProps["bypassed"] = boolProperty("True jeśli blok ma być wyłączony (bypass).")
        tools.append(ToolDefinition(
            name: "set_bypass",
            description: "Włącza lub wyłącza (bypass) określony blok efektów.",
            inputSchema: schema(properties: setBypassProps, required: targetReq + ["block", "bypassed"]),
            kind: .write
        ))

        var setNameProps = targetProps
        setNameProps["name"] = stringProperty("Nowa nazwa patcha (maksymalna długość zależy od profilu).")
        tools.append(ToolDefinition(
            name: "set_name",
            description: "Zmienia nazwę wyświetlaną patcha.",
            inputSchema: schema(properties: setNameProps, required: targetReq + ["name"]),
            kind: .write
        ))

        var setBpmProps = targetProps
        setBpmProps["bpm"] = integerProperty("Tempo BPM.")
        tools.append(ToolDefinition(
            name: "set_bpm",
            description: "Zmienia tempo BPM patcha.",
            inputSchema: schema(properties: setBpmProps, required: targetReq + ["bpm"]),
            kind: .write
        ))

        var setNamedFieldProps = targetProps
        setNamedFieldProps["field"] = stringProperty("Nazwa pola globalnego (np. send, return).")
        setNamedFieldProps["value"] = integerProperty("Wartość pola.")
        tools.append(ToolDefinition(
            name: "set_named_field",
            description: "Modyfikuje globalne pole nazwane (np. send, return).",
            inputSchema: schema(properties: setNamedFieldProps, required: targetReq + ["field", "value"]),
            kind: .write
        ))

        tools.append(ToolDefinition(
            name: "clear_ir",
            description: "Usuwa osadzony plik IR z wybranego patcha.",
            inputSchema: schema(properties: targetProps, required: targetReq),
            kind: .write
        ))

        if variant == .library {
            tools.append(ToolDefinition(
                name: "duplicate_patch",
                description: "Tworzy kopię wybranego patcha w bibliotece i zwraca jej ID. Działa tylko w trybie bibliotecznym.",
                inputSchema: schema(properties: targetProps, required: targetReq),
                kind: .write
            ))

            tools.append(ToolDefinition(
                name: "revert_last_agent_action",
                description: "Cofa ostatnią operację mutującą agenta w bieżącej sesji. Działa tylko w trybie bibliotecznym.",
                inputSchema: schema(properties: [:]),
                kind: .write
            ))
        }

        // --- UI TOOLS ---
        if variant == .library {
            tools.append(ToolDefinition(
                name: "select_patch",
                description: "Przełącza widok i zaznaczenie w GUI na wskazany patch.",
                inputSchema: schema(properties: ["patchID": stringProperty("ID patcha do zaznaczenia")], required: ["patchID"]),
                kind: .read
            ))
        }

        // --- FILESYSTEM TOOLS ---
        var setIRProps = targetProps
        if variant == .library {
            setIRProps["wavPath"] = stringProperty("Ścieżka bezwzględna do pliku WAV z zatwierdzonego katalogu.")
        } else {
            setIRProps["wav"] = stringProperty("Ścieżka bezwzględna do pliku WAV.")
        }
        setIRProps["name"] = stringProperty("Nazwa IR (maks. 32 bajty).")
        tools.append(ToolDefinition(
            name: "set_ir",
            description: "Osadza plik IR z formatu WAV we wskazanym patchu.",
            inputSchema: schema(properties: setIRProps, required: targetReq + (variant == .library ? ["wavPath", "name"] : ["wav", "name"])),
            kind: .filesystem
        ))

        if variant == .library {
            tools.append(ToolDefinition(
                name: "import_patch",
                description: "Importuje pojedynczy plik lub zestaw 36 patchy z zatwierdzonej ścieżki do biblioteki.",
                inputSchema: schema(properties: ["path": stringProperty("Ścieżka bezwzględna do pliku")], required: ["path"]),
                kind: .filesystem
            ))

            var exportProps = targetProps
            exportProps["destinationPath"] = stringProperty("Ścieżka katalogu lub pliku do eksportu.")
            tools.append(ToolDefinition(
                name: "export_patch",
                description: "Eksportuje wybrany patch do nowego pliku w zatwierdzonym katalogu.",
                inputSchema: schema(properties: exportProps, required: targetReq + ["destinationPath"]),
                kind: .filesystem
            ))

            tools.append(ToolDefinition(
                name: "list_files",
                description: "Wypisuje listę plików w zatwierdzonym katalogu.",
                inputSchema: schema(properties: ["path": stringProperty("Ścieżka bezwzględna do katalogu.")], required: ["path"]),
                kind: .filesystem
            ))
        }

        // --- DESTRUCTIVE TOOLS ---
        if variant == .library {
            tools.append(ToolDefinition(
                name: "delete_patch",
                description: "Przenosi patch do kosza sesji (miękkie usunięcie).",
                inputSchema: schema(properties: targetProps, required: targetReq),
                kind: .destructive
            ))

            tools.append(ToolDefinition(
                name: "revert_session",
                description: "Cofa wszystkie zmiany wprowadzone w bieżącej sesji rozmowy.",
                inputSchema: schema(properties: [:]),
                kind: .destructive
            ))
        }

        return tools
    }

    public func anthropicFormat() -> [String: JSONValue] {
        return [
            "name": .string(name),
            "description": .string(description),
            "input_schema": inputSchema
        ]
    }

    public func openAIFormat() -> [String: JSONValue] {
        return [
            "type": .string("function"),
            "function": .object([
                "name": .string(name),
                "description": .string(description),
                "parameters": inputSchema
            ])
        ]
    }
}

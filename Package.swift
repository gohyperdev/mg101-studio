// swift-tools-version: 6.0
import PackageDescription

let package = Package(
    name: "MG101Studio",
    platforms: [.macOS(.v14)],
    products: [
        .library(name: "MG101Core", targets: ["MG101Core"]),
        .executable(name: "MG101Studio", targets: ["MG101Studio"]),
        .executable(name: "MG101MCP", targets: ["MG101MCP"]),
    ],
    dependencies: [
        .package(
            url: "https://github.com/modelcontextprotocol/swift-sdk.git",
            from: "0.12.1"
        ),
    ],
    targets: [
        .target(
            name: "MG101Core",
            resources: [.process("Resources")]
        ),
        .target(
            name: "MG101Tools",
            dependencies: ["MG101Core"]
        ),
        .executableTarget(
            name: "MG101Studio",
            dependencies: ["MG101Core", "MG101Tools"]
        ),
        .executableTarget(
            name: "MG101MCP",
            dependencies: [
                "MG101Core",
                "MG101Tools",
                .product(name: "MCP", package: "swift-sdk"),
            ]
        ),
        .testTarget(
            name: "MG101CoreTests",
            dependencies: ["MG101Core"]
        ),
        .testTarget(
            name: "MG101ToolsTests",
            dependencies: ["MG101Tools", "MG101Core"]
        ),
        .testTarget(
            name: "MG101StudioTests",
            dependencies: ["MG101Studio", "MG101Core", "MG101Tools"]
        ),
    ]
)

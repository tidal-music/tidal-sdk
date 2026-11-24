// swift-tools-version: 6.0

import PackageDescription

let package = Package(
	name: "tidal-sdk",
	platforms: [
		.iOS(.v18),
		.macOS(.v15),
	],
	products: [
		.library(
			name: "PlayerRS",
			targets: ["PlayerRS"]
		),
	],
	targets: [
		.binaryTarget(
			name: "PlayerFFI",
			path: "platforms/ios/PlayerFFI.xcframework"
		),
		.target(
			name: "PlayerRS",
			dependencies: ["PlayerFFI"],
			path: "platforms/ios/Sources/PlayerRS"
		),
		.testTarget(
			name: "PlayerRSTests",
			dependencies: ["PlayerRS"],
			path: "platforms/ios/Tests/PlayerRSTests"
		),
	],
	// The generated bindings do not build under Swift 6 strict concurrency yet.
	swiftLanguageModes: [.v5]
)

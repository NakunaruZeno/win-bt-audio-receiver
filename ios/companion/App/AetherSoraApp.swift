import SwiftUI

/**
 * Entry point for the AetherSora iOS / iPadOS Companion App.
 */
@main
struct AetherSoraApp: App {
    @StateObject private var viewModel = CompanionViewModel()

    var body: some Scene {
        WindowGroup {
            ContentView(viewModel: viewModel)
                .preferredColorScheme(.dark)
                .tint(CelestialTheme.accentSky)
        }
    }
}

import SwiftUI

@main
struct SF2PlayerApp: App {
    @State private var player = Player()

    var body: some Scene {
        WindowGroup {
            ContentView()
                .environment(player)
        }
        #if os(macOS)
        .defaultSize(width: 520, height: 820)
        .windowResizability(.contentMinSize)
        #endif
    }
}

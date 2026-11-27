import SwiftUI

/**
 * Shared Celestial Sky Minimalist design tokens synchronized across Windows, Android, and iOS.
 */
enum CelestialTheme {
    static let bgTop = Color(hex: 0x091026)
    static let bgBase = Color(hex: 0x0D1633)
    static let bgBottom = Color(hex: 0x0A122C)

    static let surfaceCard = Color(hex: 0x121E42)
    static let surfaceSelected = Color(hex: 0x182957)
    static let surfaceTonal = Color(hex: 0x162550)
    static let surfaceInput = Color(hex: 0x0E1838)

    static let borderSubtle = Color(hex: 0x22366E)
    static let borderTonal = Color(hex: 0x2A438A)
    static let borderActive = Color(hex: 0x38BDF8)

    static let accentSky = Color(hex: 0x38BDF8)
    static let accentGold = Color(hex: 0xFACC15)
    static let statusLive = Color(hex: 0x34D399)
    static let statusError = Color(hex: 0xFB7185)

    static let textPrimary = Color(hex: 0xF8FAFC)
    static let textSecondary = Color(hex: 0x94B3D8)
    static let textTertiary = Color(hex: 0x7390BA)
    static let textOnAccent = Color(hex: 0x081126)

    static let cardRadius: CGFloat = 16
    static let inputRadius: CGFloat = 14
    static let badgeRadius: CGFloat = 12
}

extension Color {
    init(hex: UInt32) {
        let r = Double((hex >> 16) & 0xFF) / 255.0
        let g = Double((hex >> 8) & 0xFF) / 255.0
        let b = Double(hex & 0xFF) / 255.0
        self.init(.sRGB, red: r, green: g, blue: b, opacity: 1.0)
    }
}

import SwiftUI

/**
 * Celestial Chrono-Star Audio Orb displaying idle, LAN scan sweep, and RMS-reactive streaming
 * rings while honoring Reduce Motion and stopping frame updates whenever idle.
 */
struct CelestialOrbView: View {
    enum OrbState: Equatable {
        case idle
        case scanning
        case streaming
    }

    let state: OrbState
    let audioLevel: Float

    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    var body: some View {
        Group {
            if state == .idle || reduceMotion {
                orbCanvas(now: Date(timeIntervalSinceReferenceDate: 0))
            } else {
                TimelineView(.animation) { context in
                    orbCanvas(now: context.date)
                }
            }
        }
        .frame(width: 104, height: 104)
        .accessibilityHidden(true)
    }

    private func orbCanvas(now: Date) -> some View {
        Canvas { context, size in
            let cx = size.width * 0.5
            let cy = size.height * 0.5
            let minDim = min(size.width, size.height)
            let baseRadius = minDim * 0.34
            let level = CGFloat(max(0, min(1, audioLevel)))
            let expansion: CGFloat = (state == .streaming) ? level * 7.0 : 0.0

            if state == .streaming {
                let haloRadius = baseRadius + expansion + 5.0
                let haloRect = CGRect(
                    x: cx - haloRadius,
                    y: cy - haloRadius,
                    width: haloRadius * 2,
                    height: haloRadius * 2)
                context.fill(
                    Path(ellipseIn: haloRect),
                    with: .color(CelestialTheme.statusLive.opacity(0.11 + Double(level) * 0.17)))
            } else if state == .scanning {
                let haloRadius = baseRadius + 4.0
                let haloRect = CGRect(
                    x: cx - haloRadius,
                    y: cy - haloRadius,
                    width: haloRadius * 2,
                    height: haloRadius * 2)
                context.fill(
                    Path(ellipseIn: haloRect),
                    with: .color(CelestialTheme.accentSky.opacity(0.12)))
            }

            let trackRadius = baseRadius + expansion
            let trackRect = CGRect(
                x: cx - trackRadius,
                y: cy - trackRadius,
                width: trackRadius * 2,
                height: trackRadius * 2)
            context.stroke(
                Path(ellipseIn: trackRect),
                with: .color(CelestialTheme.borderSubtle),
                lineWidth: 1.5)

            if state == .scanning {
                let sweepRadius = baseRadius + 4.0
                if reduceMotion {
                    let ringRect = CGRect(
                        x: cx - sweepRadius,
                        y: cy - sweepRadius,
                        width: sweepRadius * 2,
                        height: sweepRadius * 2)
                    context.stroke(
                        Path(ellipseIn: ringRect),
                        with: .color(CelestialTheme.accentSky),
                        lineWidth: 2.4)
                } else {
                    let deg = (now.timeIntervalSinceReferenceDate * 240.0)
                        .truncatingRemainder(dividingBy: 360.0)
                    var sweep = Path()
                    sweep.addArc(
                        center: CGPoint(x: cx, y: cy),
                        radius: sweepRadius,
                        startAngle: .degrees(deg - 90.0),
                        endAngle: .degrees(deg + 6.0),
                        clockwise: false)
                    context.stroke(
                        sweep,
                        with: .color(CelestialTheme.accentSky),
                        style: StrokeStyle(lineWidth: 2.4, lineCap: .round))
                }
            } else if state == .streaming {
                context.stroke(
                    Path(ellipseIn: trackRect),
                    with: .color(CelestialTheme.statusLive),
                    lineWidth: 2.2 + level * 1.4)
            }

            drawStarlightPinpoints(context: &context, cx: cx, cy: cy, ringRadius: baseRadius + 8.0)
            drawChronoStarMark(context: &context, cx: cx, cy: cy, diameter: minDim * 0.50)
        }
    }

    private func drawStarlightPinpoints(
        context: inout GraphicsContext,
        cx: CGFloat,
        cy: CGFloat,
        ringRadius: CGFloat
    ) {
        let primaryColor = state == .streaming
            ? CelestialTheme.accentGold
            : CelestialTheme.accentSky.opacity(0.67)
        let secondaryColor = CelestialTheme.accentGold.opacity(0.55)

        let points: [(CGFloat, CGFloat, CGFloat, Color)] = [
            (-0.72, -0.72, 1.8, primaryColor),
            (0.78, 0.68, 1.5, primaryColor),
            (0.74, -0.70, 1.3, secondaryColor),
            (-0.76, 0.72, 1.3, secondaryColor)
        ]

        for (dx, dy, r, color) in points {
            let pt = CGRect(
                x: cx + ringRadius * dx - r,
                y: cy + ringRadius * dy - r,
                width: r * 2,
                height: r * 2)
            context.fill(Path(ellipseIn: pt), with: .color(color))
        }
    }

    private func drawChronoStarMark(
        context: inout GraphicsContext,
        cx: CGFloat,
        cy: CGFloat,
        diameter: CGFloat
    ) {
        let s = diameter / 256.0
        let ox = cx - 128.0 * s
        let oy = cy - 128.0 * s
        let center = CGPoint(x: cx, y: cy)
        let rOut = 94.0 * s
        let rIn = 66.0 * s

        var arc1 = Path()
        arc1.addArc(
            center: center,
            radius: rOut,
            startAngle: .degrees(-120),
            endAngle: .degrees(30),
            clockwise: false)
        arc1.addArc(
            center: center,
            radius: rIn,
            startAngle: .degrees(30),
            endAngle: .degrees(-120),
            clockwise: true)
        arc1.closeSubpath()
        context.fill(arc1, with: .color(CelestialTheme.accentSky))

        var arc2 = Path()
        arc2.addArc(
            center: center,
            radius: rOut,
            startAngle: .degrees(60),
            endAngle: .degrees(210),
            clockwise: false)
        arc2.addArc(
            center: center,
            radius: rIn,
            startAngle: .degrees(210),
            endAngle: .degrees(60),
            clockwise: true)
        arc2.closeSubpath()
        context.fill(arc2, with: .color(CelestialTheme.accentSky))

        var star = Path()
        star.move(to: CGPoint(x: ox + 128 * s, y: oy + 64 * s))
        star.addQuadCurve(
            to: CGPoint(x: ox + 192 * s, y: oy + 128 * s),
            control: CGPoint(x: ox + 128 * s, y: oy + 115 * s))
        star.addQuadCurve(
            to: CGPoint(x: ox + 128 * s, y: oy + 192 * s),
            control: CGPoint(x: ox + 128 * s, y: oy + 141 * s))
        star.addQuadCurve(
            to: CGPoint(x: ox + 64 * s, y: oy + 128 * s),
            control: CGPoint(x: ox + 128 * s, y: oy + 141 * s))
        star.addQuadCurve(
            to: CGPoint(x: ox + 128 * s, y: oy + 64 * s),
            control: CGPoint(x: ox + 128 * s, y: oy + 115 * s))
        star.closeSubpath()
        context.fill(star, with: .color(CelestialTheme.accentGold))

        var d1 = Path()
        d1.move(to: CGPoint(x: ox + 64 * s, y: oy + 50 * s))
        d1.addLine(to: CGPoint(x: ox + 78 * s, y: oy + 64 * s))
        d1.addLine(to: CGPoint(x: ox + 64 * s, y: oy + 78 * s))
        d1.addLine(to: CGPoint(x: ox + 50 * s, y: oy + 64 * s))
        d1.closeSubpath()
        context.fill(d1, with: .color(CelestialTheme.accentGold))

        var d2 = Path()
        d2.move(to: CGPoint(x: ox + 192 * s, y: oy + 178 * s))
        d2.addLine(to: CGPoint(x: ox + 206 * s, y: oy + 192 * s))
        d2.addLine(to: CGPoint(x: ox + 192 * s, y: oy + 206 * s))
        d2.addLine(to: CGPoint(x: ox + 178 * s, y: oy + 192 * s))
        d2.closeSubpath()
        context.fill(d2, with: .color(CelestialTheme.accentGold))
    }
}

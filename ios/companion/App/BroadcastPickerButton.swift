import ReplayKit
import SwiftUI
import UIKit

/**
 * Wraps `RPSystemBroadcastPickerView` so its internal UIButton fills 100% of our custom
 * full-pill CTA bounds while resolving `.BroadcastUpload` dynamically across bundle ID rewrites.
 */
struct BroadcastPickerButton: UIViewRepresentable {
    var onPrepareTarget: () -> Void

    func makeCoordinator() -> Coordinator {
        Coordinator(onPrepareTarget: onPrepareTarget)
    }

    func makeUIView(context: Context) -> BroadcastPickerContainerView {
        let container = BroadcastPickerContainerView()
        container.onBeforeOpen = {
            context.coordinator.onPrepareTarget()
        }
        return container
    }

    func updateUIView(_ uiView: BroadcastPickerContainerView, context: Context) {
        context.coordinator.onPrepareTarget = onPrepareTarget
        uiView.onBeforeOpen = {
            context.coordinator.onPrepareTarget()
        }
        uiView.refreshExtensionIdentifier()
    }

    final class Coordinator {
        var onPrepareTarget: () -> Void

        init(onPrepareTarget: @escaping () -> Void) {
            self.onPrepareTarget = onPrepareTarget
        }
    }
}

final class BroadcastPickerContainerView: UIView {
    private let picker = RPSystemBroadcastPickerView(frame: .zero)
    var onBeforeOpen: (() -> Void)?

    override init(frame: CGRect) {
        super.init(frame: frame)
        backgroundColor = .clear
        picker.showsMicrophoneButton = false
        picker.backgroundColor = .clear
        refreshExtensionIdentifier()
        addSubview(picker)
        hookPickerButtons()
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) {
        fatalError("init(coder:) has not been implemented")
    }

    func refreshExtensionIdentifier() {
        let hostId = Bundle.main.bundleIdentifier ?? "com.aethersora.companion"
        picker.preferredExtension = "\(hostId).BroadcastUpload"
    }

    override func layoutSubviews() {
        super.layoutSubviews()
        picker.frame = bounds
        hookPickerButtons()
    }

    private func hookPickerButtons() {
        for subview in picker.subviews {
            if let btn = subview as? UIButton {
                btn.frame = picker.bounds
                btn.setImage(nil, for: .normal)
                btn.setTitle(nil, for: .normal)
                btn.backgroundColor = .clear
                btn.removeTarget(self, action: #selector(handleTouchDown), for: .touchDown)
                btn.addTarget(self, action: #selector(handleTouchDown), for: .touchDown)
            }
        }
    }

    @objc private func handleTouchDown() {
        UIImpactFeedbackGenerator(style: .light).impactOccurred()
        onBeforeOpen?()
    }
}

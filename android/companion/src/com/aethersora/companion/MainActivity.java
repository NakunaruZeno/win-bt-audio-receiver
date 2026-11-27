package com.aethersora.companion;

import android.Manifest;
import android.app.Activity;
import android.content.BroadcastReceiver;
import android.content.Context;
import android.content.Intent;
import android.content.IntentFilter;
import android.content.SharedPreferences;
import android.content.pm.PackageManager;
import android.content.res.ColorStateList;
import android.graphics.Insets;
import android.media.projection.MediaProjectionManager;
import android.os.Build;
import android.os.Bundle;
import android.provider.Settings;
import android.text.Editable;
import android.text.TextWatcher;
import android.util.Log;
import android.view.HapticFeedbackConstants;
import android.view.LayoutInflater;
import android.view.View;
import android.view.Window;
import android.view.WindowInsets;
import android.view.WindowInsetsController;
import android.view.inputmethod.EditorInfo;
import android.view.inputmethod.InputMethodManager;
import android.widget.EditText;
import android.widget.ImageView;
import android.widget.LinearLayout;
import android.widget.ScrollView;
import android.widget.Switch;
import android.widget.TextView;

import java.io.PrintWriter;
import java.io.StringWriter;
import java.util.ArrayList;
import java.util.List;

/**
 * Celestial Sky Minimalist Companion UI for Android 10+.
 * Discovers AetherSora PCs over Wi-Fi LAN and streams 48 kHz Lossless PCM with one tap.
 */
public final class MainActivity extends Activity {
    private static final String TAG = "AetherSora";
    private static final int REQ_RECORD_AUDIO = 101;
    private static final int REQ_MEDIA_PROJECTION = 102;
    private static final String PREFS = "aethersora_prefs";
    private static final String KEY_LAST_IP = "last_pc_ip";
    private static final String KEY_AUTO_MUTE = "auto_mute_phone";

    private CelestialOrbView orbView;
    private TextView txtStatusTitle;
    private TextView txtStatusSubtitle;
    private View viewStatusDot;
    private LinearLayout btnScan;
    private TextView txtScanLabel;
    private LinearLayout pcListContainer;
    private LinearLayout emptyPcCard;
    private LinearLayout btnManualIpToggle;
    private ImageView imgManualChevron;
    private LinearLayout manualIpContainer;
    private EditText editPcIp;
    private Switch switchAutoMute;
    private LinearLayout rowAutoMute;
    private LinearLayout btnToggle;
    private ImageView imgToggleIcon;
    private TextView txtToggleLabel;

    private final List<LanDiscovery.DiscoveredPc> discovered = new ArrayList<>();
    private boolean scanning = false;
    private boolean manualIpExpanded = false;
    private boolean userToggledManualIp = false;
    private boolean receiversRegistered = false;
    private boolean uiReady = false;

    private final BroadcastReceiver statusReceiver = new BroadcastReceiver() {
        @Override
        public void onReceive(Context context, Intent intent) {
            if (!uiReady) {
                return;
            }
            boolean active = intent.getBooleanExtra(AudioCaptureService.EXTRA_RUNNING, false);
            int statusCode = intent.getIntExtra(
                    AudioCaptureService.EXTRA_STATE,
                    active ? AudioCaptureService.STATUS_STREAMING : AudioCaptureService.STATUS_IDLE);
            String title = intent.getStringExtra(AudioCaptureService.EXTRA_TITLE);
            String msg = intent.getStringExtra(AudioCaptureService.EXTRA_MESSAGE);

            if (title != null) {
                txtStatusTitle.setText(title);
            } else {
                txtStatusTitle.setText(active ? "Streaming Lossless" : "Ready to stream");
            }
            if (msg != null) {
                txtStatusSubtitle.setText(msg);
            }
            applyStatusVisuals(active, statusCode);
        }
    };

    private final BroadcastReceiver levelReceiver = new BroadcastReceiver() {
        @Override
        public void onReceive(Context context, Intent intent) {
            if (uiReady && AudioCaptureService.isStreaming) {
                float level = intent.getFloatExtra(AudioCaptureService.EXTRA_LEVEL, 0f);
                orbView.setAudioLevel(level);
            }
        }
    };

    private static final class Api30Compat {
        static void configureEdgeToEdge(Window window, View decor) {
            try {
                window.setDecorFitsSystemWindows(false);
            } catch (Throwable ignored) {
            }
            if (decor != null) {
                decor.post(() -> {
                    try {
                        WindowInsetsController controller = window.getInsetsController();
                        if (controller != null) {
                            controller.setSystemBarsAppearance(
                                    0,
                                    WindowInsetsController.APPEARANCE_LIGHT_STATUS_BARS
                                            | WindowInsetsController.APPEARANCE_LIGHT_NAVIGATION_BARS);
                        }
                    } catch (Throwable ignored) {
                    }
                });
            }
        }

        static int[] extractInsets(WindowInsets insets) {
            Insets bar = insets.getInsets(
                    WindowInsets.Type.systemBars()
                            | WindowInsets.Type.displayCutout()
                            | WindowInsets.Type.ime());
            return new int[] { bar.top, bar.bottom, bar.left, bar.right };
        }
    }

    @Override
    protected void onCreate(Bundle savedInstanceState) {
        super.onCreate(savedInstanceState);
        try {
            initUi();
        } catch (Throwable t) {
            showStartupError(t);
        }
    }

    private void initUi() {
        setContentView(R.layout.activity_main);
        configureEdgeToEdge();
        applyWindowInsets();

        orbView = findViewById(R.id.orbView);
        txtStatusTitle = findViewById(R.id.txtStatusTitle);
        txtStatusSubtitle = findViewById(R.id.txtStatusSubtitle);
        viewStatusDot = findViewById(R.id.viewStatusDot);
        btnScan = findViewById(R.id.btnScan);
        txtScanLabel = findViewById(R.id.txtScanLabel);
        pcListContainer = findViewById(R.id.pcListContainer);
        emptyPcCard = findViewById(R.id.emptyPcCard);
        btnManualIpToggle = findViewById(R.id.btnManualIpToggle);
        imgManualChevron = findViewById(R.id.imgManualChevron);
        manualIpContainer = findViewById(R.id.manualIpContainer);
        editPcIp = findViewById(R.id.editPcIp);
        switchAutoMute = findViewById(R.id.switchAutoMute);
        rowAutoMute = findViewById(R.id.rowAutoMute);
        btnToggle = findViewById(R.id.btnToggle);
        imgToggleIcon = findViewById(R.id.imgToggleIcon);
        txtToggleLabel = findViewById(R.id.txtToggleLabel);

        SharedPreferences prefs = getSharedPreferences(PREFS, MODE_PRIVATE);
        String savedIp = prefs.getString(KEY_LAST_IP, "");
        editPcIp.setText(savedIp);
        switchAutoMute.setChecked(prefs.getBoolean(KEY_AUTO_MUTE, true));
        switchAutoMute.setOnCheckedChangeListener((btn, isChecked) -> {
            btn.performHapticFeedback(HapticFeedbackConstants.CLOCK_TICK);
            prefs.edit().putBoolean(KEY_AUTO_MUTE, isChecked).apply();
        });
        rowAutoMute.setOnClickListener(v -> switchAutoMute.performClick());

        editPcIp.addTextChangedListener(new TextWatcher() {
            @Override
            public void beforeTextChanged(CharSequence s, int start, int count, int after) {
            }

            @Override
            public void onTextChanged(CharSequence s, int start, int before, int count) {
            }

            @Override
            public void afterTextChanged(Editable s) {
                renderPcCards();
            }
        });

        editPcIp.setOnEditorActionListener((v, actionId, event) -> {
            if (actionId == EditorInfo.IME_ACTION_DONE) {
                InputMethodManager imm = (InputMethodManager) getSystemService(Context.INPUT_METHOD_SERVICE);
                if (imm != null) {
                    imm.hideSoftInputFromWindow(v.getWindowToken(), 0);
                }
                v.clearFocus();
                return true;
            }
            return false;
        });

        btnManualIpToggle.setOnClickListener(v -> {
            v.performHapticFeedback(HapticFeedbackConstants.KEYBOARD_TAP);
            userToggledManualIp = true;
            setManualIpExpanded(!manualIpExpanded, true);
        });

        btnScan.setOnClickListener(v -> {
            v.performHapticFeedback(HapticFeedbackConstants.KEYBOARD_TAP);
            startLanScan();
        });

        btnToggle.setOnClickListener(v -> {
            v.performHapticFeedback(HapticFeedbackConstants.KEYBOARD_TAP);
            onToggleClicked();
        });

        uiReady = true;
        syncInitialState();
        startLanScan();
    }

    private void showStartupError(Throwable t) {
        uiReady = false;
        Log.e(TAG, "Startup failed", t);
        StringWriter sw = new StringWriter();
        t.printStackTrace(new PrintWriter(sw));

        float density = getResources().getDisplayMetrics().density;
        int pad = Math.round(20f * density);
        ScrollView scroll = new ScrollView(this);
        scroll.setBackgroundColor(0xFF0D1633);

        TextView tv = new TextView(this);
        tv.setPadding(pad, Math.round(48f * density), pad, pad);
        tv.setTextColor(0xFFF8FAFC);
        tv.setTextSize(13f);
        tv.setText("AetherSora startup error:\n\n" + t + "\n\n" + sw);
        scroll.addView(tv);
        setContentView(scroll);
    }

    private void configureEdgeToEdge() {
        Window window = getWindow();
        if (window == null) {
            return;
        }
        View decor = window.getDecorView();
        if (Build.VERSION.SDK_INT >= 30) {
            Api30Compat.configureEdgeToEdge(window, decor);
        } else if (decor != null) {
            decor.setSystemUiVisibility(
                    View.SYSTEM_UI_FLAG_LAYOUT_STABLE
                            | View.SYSTEM_UI_FLAG_LAYOUT_FULLSCREEN
                            | View.SYSTEM_UI_FLAG_LAYOUT_HIDE_NAVIGATION);
        }
    }

    private void applyWindowInsets() {
        View root = findViewById(R.id.rootLayout);
        View topBar = findViewById(R.id.topBar);
        View bottomDock = findViewById(R.id.bottomDock);
        float density = getResources().getDisplayMetrics().density;

        final int sidePad = Math.round(20f * density);
        final int topBase = Math.round(12f * density);
        final int topBottomPad = Math.round(8f * density);
        final int dockTopPad = Math.round(10f * density);
        final int dockBaseBottom = Math.round(16f * density);

        root.setOnApplyWindowInsetsListener((v, insets) -> {
            int topInset;
            int bottomInset;
            int leftInset;
            int rightInset;
            if (Build.VERSION.SDK_INT >= 30) {
                int[] extracted = Api30Compat.extractInsets(insets);
                topInset = extracted[0];
                bottomInset = extracted[1];
                leftInset = extracted[2];
                rightInset = extracted[3];
            } else {
                topInset = insets.getSystemWindowInsetTop();
                bottomInset = insets.getSystemWindowInsetBottom();
                leftInset = insets.getSystemWindowInsetLeft();
                rightInset = insets.getSystemWindowInsetRight();
            }

            topBar.setPadding(
                    sidePad + leftInset,
                    topBase + topInset,
                    sidePad + rightInset,
                    topBottomPad);
            bottomDock.setPadding(
                    sidePad + leftInset,
                    dockTopPad,
                    sidePad + rightInset,
                    dockBaseBottom + bottomInset);
            return insets;
        });
        root.requestApplyInsets();
    }

    private void syncInitialState() {
        if (!uiReady) {
            return;
        }
        boolean active = AudioCaptureService.isStreaming;
        if (active) {
            txtStatusTitle.setText(AudioCaptureService.lastStatusTitle);
            txtStatusSubtitle.setText(AudioCaptureService.lastStatusSubtitle);
            applyStatusVisuals(true, AudioCaptureService.STATUS_STREAMING);
        } else {
            txtStatusTitle.setText("Ready to stream");
            txtStatusSubtitle.setText("Scanning Wi-Fi LAN for PCs...");
            applyStatusVisuals(false, AudioCaptureService.STATUS_IDLE);
        }
    }

    @Override
    protected void onResume() {
        super.onResume();
        if (!uiReady) {
            return;
        }
        if (!receiversRegistered) {
            IntentFilter statusFilter = new IntentFilter(AudioCaptureService.ACTION_STATUS);
            IntentFilter levelFilter = new IntentFilter(AudioCaptureService.ACTION_LEVEL);
            if (Build.VERSION.SDK_INT >= 33) {
                registerReceiver(statusReceiver, statusFilter, Context.RECEIVER_NOT_EXPORTED);
                registerReceiver(levelReceiver, levelFilter, Context.RECEIVER_NOT_EXPORTED);
            } else {
                registerReceiver(statusReceiver, statusFilter);
                registerReceiver(levelReceiver, levelFilter);
            }
            receiversRegistered = true;
        }
        syncInitialState();
    }

    @Override
    protected void onPause() {
        if (receiversRegistered) {
            try {
                unregisterReceiver(statusReceiver);
            } catch (Throwable ignored) {
            }
            try {
                unregisterReceiver(levelReceiver);
            } catch (Throwable ignored) {
            }
            receiversRegistered = false;
        }
        super.onPause();
    }

    private void startLanScan() {
        if (scanning || !uiReady) {
            return;
        }
        scanning = true;
        btnScan.setEnabled(false);
        btnScan.setAlpha(0.6f);
        txtScanLabel.setText("Scanning");
        if (!AudioCaptureService.isStreaming) {
            orbView.setState(CelestialOrbView.STATE_SCANNING);
            txtStatusTitle.setText("Searching LAN");
            txtStatusSubtitle.setText("Listening for AetherSora PC on Wi-Fi...");
            applyStatusDot(AudioCaptureService.STATUS_IDLE);
        }

        new Thread(() -> {
            List<LanDiscovery.DiscoveredPc> found = LanDiscovery.scan(1200);
            runOnUiThread(() -> {
                if (!uiReady || isDestroyed()) {
                    return;
                }
                scanning = false;
                discovered.clear();
                discovered.addAll(found);

                btnScan.setEnabled(true);
                btnScan.setAlpha(1.0f);
                txtScanLabel.setText("Scan");

                if (!found.isEmpty()) {
                    if (editPcIp.getText().toString().trim().isEmpty()) {
                        editPcIp.setText(found.get(0).ip);
                    }
                    if (!userToggledManualIp) {
                        setManualIpExpanded(false, false);
                    }
                } else if (!userToggledManualIp) {
                    setManualIpExpanded(true, false);
                }

                renderPcCards();

                if (!AudioCaptureService.isStreaming) {
                    if (!found.isEmpty()) {
                        LanDiscovery.DiscoveredPc activeTarget = findSelectedOrFirst();
                        txtStatusTitle.setText("Ready to stream");
                        txtStatusSubtitle.setText("Target: " + activeTarget.hostname + " • " + activeTarget.ip);
                        applyStatusVisuals(false, AudioCaptureService.STATUS_IDLE);
                    } else {
                        String typed = editPcIp.getText().toString().trim();
                        if (!typed.isEmpty()) {
                            txtStatusTitle.setText("Saved PC ready");
                            txtStatusSubtitle.setText("Will send audio to " + typed);
                        } else {
                            txtStatusTitle.setText("No PC discovered");
                            txtStatusSubtitle.setText("Enter your PC's Wi-Fi IP below");
                        }
                        applyStatusVisuals(false, AudioCaptureService.STATUS_IDLE);
                    }
                }
            });
        }, "aether-scan").start();
    }

    private LanDiscovery.DiscoveredPc findSelectedOrFirst() {
        String currentIp = editPcIp.getText().toString().trim();
        for (LanDiscovery.DiscoveredPc pc : discovered) {
            if (pc.ip.equals(currentIp)) {
                return pc;
            }
        }
        return discovered.get(0);
    }

    private void renderPcCards() {
        if (!uiReady) {
            return;
        }
        pcListContainer.removeAllViews();
        if (discovered.isEmpty()) {
            emptyPcCard.setVisibility(View.VISIBLE);
            return;
        }
        emptyPcCard.setVisibility(View.GONE);

        String selectedIp = editPcIp.getText().toString().trim();
        LayoutInflater inflater = LayoutInflater.from(this);
        float density = getResources().getDisplayMetrics().density;
        int gapPx = Math.round(8f * density);

        for (int i = 0; i < discovered.size(); i++) {
            LanDiscovery.DiscoveredPc pc = discovered.get(i);
            View card = inflater.inflate(R.layout.item_pc_card, pcListContainer, false);
            LinearLayout.LayoutParams lp = (LinearLayout.LayoutParams) card.getLayoutParams();
            lp.bottomMargin = (i == discovered.size() - 1) ? 0 : gapPx;
            card.setLayoutParams(lp);

            TextView txtName = card.findViewById(R.id.txtPcName);
            TextView txtAddr = card.findViewById(R.id.txtPcAddress);
            ImageView imgIcon = card.findViewById(R.id.imgPcIcon);
            ImageView imgSelected = card.findViewById(R.id.imgPcSelected);

            txtName.setText(pc.hostname);
            txtAddr.setText(pc.ip + ":" + pc.audioPort);

            boolean selected = pc.ip.equals(selectedIp);
            card.setSelected(selected);
            imgSelected.setVisibility(selected ? View.VISIBLE : View.GONE);
            int iconColor = getColor(selected ? R.color.accent_sky : R.color.text_secondary);
            imgIcon.setImageTintList(ColorStateList.valueOf(iconColor));
            card.setContentDescription(pc.hostname + " at " + pc.ip + (selected ? ", selected" : ""));

            card.setOnClickListener(v -> {
                v.performHapticFeedback(HapticFeedbackConstants.KEYBOARD_TAP);
                editPcIp.setText(pc.ip);
                getSharedPreferences(PREFS, MODE_PRIVATE).edit().putString(KEY_LAST_IP, pc.ip).apply();
                if (!AudioCaptureService.isStreaming) {
                    txtStatusTitle.setText("Ready to stream");
                    txtStatusSubtitle.setText("Target: " + pc.hostname + " • " + pc.ip);
                    applyStatusDot(AudioCaptureService.STATUS_IDLE);
                }
                renderPcCards();
            });

            pcListContainer.addView(card);
        }
    }

    private void setManualIpExpanded(boolean expanded, boolean animate) {
        manualIpExpanded = expanded;
        manualIpContainer.setVisibility(expanded ? View.VISIBLE : View.GONE);
        float targetRot = expanded ? 180f : 0f;
        if (animate && !isReducedMotionEnabled()) {
            imgManualChevron.animate().rotation(targetRot).setDuration(180L).start();
        } else {
            imgManualChevron.setRotation(targetRot);
        }
    }

    private boolean isReducedMotionEnabled() {
        try {
            return Settings.Global.getFloat(
                    getContentResolver(),
                    Settings.Global.ANIMATOR_DURATION_SCALE,
                    1.0f) == 0f;
        } catch (Throwable ignored) {
            return false;
        }
    }

    private void onToggleClicked() {
        if (AudioCaptureService.isStreaming) {
            Intent stop = new Intent(this, AudioCaptureService.class);
            stop.setAction(AudioCaptureService.ACTION_STOP);
            startService(stop);
            return;
        }

        String ip = editPcIp.getText().toString().trim();
        if (!isPlausibleIpv4(ip)) {
            setManualIpExpanded(true, true);
            editPcIp.requestFocus();
            txtStatusTitle.setText("PC IP required");
            txtStatusSubtitle.setText("Tap a discovered PC or enter a valid IPv4 address");
            applyStatusVisuals(false, AudioCaptureService.STATUS_ERROR);
            return;
        }

        getSharedPreferences(PREFS, MODE_PRIVATE).edit().putString(KEY_LAST_IP, ip).apply();

        if (checkSelfPermission(Manifest.permission.RECORD_AUDIO) != PackageManager.PERMISSION_GRANTED) {
            List<String> perms = new ArrayList<>();
            perms.add(Manifest.permission.RECORD_AUDIO);
            if (Build.VERSION.SDK_INT >= 33) {
                perms.add(Manifest.permission.POST_NOTIFICATIONS);
            }
            requestPermissions(perms.toArray(new String[0]), REQ_RECORD_AUDIO);
            return;
        }

        requestProjectionConsent();
    }

    private static boolean isPlausibleIpv4(String ip) {
        if (ip.isEmpty()) {
            return false;
        }
        String[] parts = ip.split("\\.", -1);
        if (parts.length != 4) {
            return false;
        }
        for (String part : parts) {
            if (part.isEmpty() || part.length() > 3) {
                return false;
            }
            try {
                int v = Integer.parseInt(part);
                if (v < 0 || v > 255) {
                    return false;
                }
            } catch (NumberFormatException e) {
                return false;
            }
        }
        return true;
    }

    private void requestProjectionConsent() {
        MediaProjectionManager mpm =
                (MediaProjectionManager) getSystemService(Context.MEDIA_PROJECTION_SERVICE);
        startActivityForResult(mpm.createScreenCaptureIntent(), REQ_MEDIA_PROJECTION);
    }

    @Override
    public void onRequestPermissionsResult(int requestCode, String[] permissions, int[] grantResults) {
        if (!uiReady) {
            return;
        }
        if (requestCode == REQ_RECORD_AUDIO
                && grantResults.length > 0
                && grantResults[0] == PackageManager.PERMISSION_GRANTED) {
            requestProjectionConsent();
        } else {
            txtStatusTitle.setText("Permission required");
            txtStatusSubtitle.setText("Allow Audio permission so Android can capture playback");
            applyStatusVisuals(false, AudioCaptureService.STATUS_ERROR);
        }
    }

    @Override
    protected void onActivityResult(int requestCode, int resultCode, Intent data) {
        if (!uiReady) {
            return;
        }
        if (requestCode == REQ_MEDIA_PROJECTION) {
            if (resultCode == RESULT_OK && data != null) {
                String ip = editPcIp.getText().toString().trim();
                Intent svc = new Intent(this, AudioCaptureService.class);
                svc.setAction(AudioCaptureService.ACTION_START);
                svc.putExtra(AudioCaptureService.EXTRA_RESULT_CODE, resultCode);
                svc.putExtra(AudioCaptureService.EXTRA_RESULT_DATA, data);
                svc.putExtra(AudioCaptureService.EXTRA_PC_IP, ip);
                svc.putExtra(AudioCaptureService.EXTRA_PC_PORT, 19480);
                svc.putExtra(AudioCaptureService.EXTRA_AUTO_MUTE, switchAutoMute.isChecked());
                startForegroundService(svc);

                txtStatusTitle.setText("Connecting...");
                txtStatusSubtitle.setText("Handshaking 48 kHz UDP with " + ip);
                applyStatusVisuals(true, AudioCaptureService.STATUS_STREAMING);
            } else {
                txtStatusTitle.setText("Capture canceled");
                txtStatusSubtitle.setText("Accept the system capture dialog when ready");
                applyStatusVisuals(false, AudioCaptureService.STATUS_IDLE);
            }
        }
    }

    private void applyStatusDot(int statusCode) {
        int dotColor;
        if (statusCode == AudioCaptureService.STATUS_STREAMING) {
            dotColor = getColor(R.color.status_live);
        } else if (statusCode == AudioCaptureService.STATUS_ERROR) {
            dotColor = getColor(R.color.status_error);
        } else {
            dotColor = getColor(R.color.accent_sky);
        }
        viewStatusDot.setBackgroundTintList(ColorStateList.valueOf(dotColor));
    }

    private void applyStatusVisuals(boolean streaming, int statusCode) {
        applyStatusDot(statusCode);
        if (streaming) {
            orbView.setState(CelestialOrbView.STATE_STREAMING);
            btnToggle.setBackgroundResource(R.drawable.bg_btn_active);
            imgToggleIcon.setImageResource(R.drawable.ic_stop);
            txtToggleLabel.setText("Disconnect");
            btnToggle.setContentDescription("Disconnect audio stream");
        } else {
            orbView.setState(scanning ? CelestialOrbView.STATE_SCANNING : CelestialOrbView.STATE_IDLE);
            btnToggle.setBackgroundResource(R.drawable.bg_btn_primary);
            imgToggleIcon.setImageResource(R.drawable.ic_waveform);
            txtToggleLabel.setText("Connect & Stream");
            btnToggle.setContentDescription("Connect and stream lossless audio to selected PC");
        }
    }
}

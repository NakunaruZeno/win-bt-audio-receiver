package com.aethersora.companion;

import android.Manifest;
import android.app.Activity;
import android.content.BroadcastReceiver;
import android.content.Context;
import android.content.Intent;
import android.content.IntentFilter;
import android.content.SharedPreferences;
import android.content.pm.PackageManager;
import android.graphics.Color;
import android.media.projection.MediaProjectionManager;
import android.os.Build;
import android.os.Bundle;
import android.view.View;
import android.view.ViewGroup;
import android.widget.ArrayAdapter;
import android.widget.Button;
import android.widget.CheckBox;
import android.widget.EditText;
import android.widget.ListView;
import android.widget.TextView;

import java.util.ArrayList;
import java.util.List;

/**
 * Single-screen Tokino Sora themed Companion UI.
 * Discovers AetherSora PCs on Wi-Fi LAN and starts 48 kHz Lossless PCM streaming with 1 tap.
 */
public final class MainActivity extends Activity {
    private static final int REQ_RECORD_AUDIO = 101;
    private static final int REQ_MEDIA_PROJECTION = 102;
    private static final String PREFS = "aethersora_prefs";
    private static final String KEY_LAST_IP = "last_pc_ip";
    private static final String KEY_AUTO_MUTE = "auto_mute_phone";

    private TextView txtStatus;
    private EditText editPcIp;
    private CheckBox chkAutoMute;
    private Button btnToggle;
    private Button btnScan;
    private ListView listPcs;

    private final List<LanDiscovery.DiscoveredPc> discovered = new ArrayList<>();
    private ArrayAdapter<LanDiscovery.DiscoveredPc> adapter;

    private final BroadcastReceiver statusReceiver = new BroadcastReceiver() {
        @Override
        public void onReceive(Context context, Intent intent) {
            boolean active = intent.getBooleanExtra(AudioCaptureService.EXTRA_RUNNING, false);
            String msg = intent.getStringExtra(AudioCaptureService.EXTRA_MESSAGE);
            if (msg != null) {
                txtStatus.setText(msg);
            }
            updateToggleButton(active);
        }
    };

    @Override
    protected void onCreate(Bundle savedInstanceState) {
        super.onCreate(savedInstanceState);
        setContentView(R.layout.activity_main);

        txtStatus = findViewById(R.id.txtStatus);
        editPcIp = findViewById(R.id.editPcIp);
        chkAutoMute = findViewById(R.id.chkAutoMute);
        btnToggle = findViewById(R.id.btnToggle);
        btnScan = findViewById(R.id.btnScan);
        listPcs = findViewById(R.id.listPcs);

        SharedPreferences prefs = getSharedPreferences(PREFS, MODE_PRIVATE);
        editPcIp.setText(prefs.getString(KEY_LAST_IP, ""));
        chkAutoMute.setChecked(prefs.getBoolean(KEY_AUTO_MUTE, true));
        chkAutoMute.setOnCheckedChangeListener((btn, isChecked) ->
                prefs.edit().putBoolean(KEY_AUTO_MUTE, isChecked).apply());

        adapter = new ArrayAdapter<LanDiscovery.DiscoveredPc>(
                this, android.R.layout.simple_list_item_1, discovered) {
            @Override
            public View getView(int position, View convertView, ViewGroup parent) {
                TextView tv = (TextView) super.getView(position, convertView, parent);
                tv.setTextColor(Color.parseColor("#F8FAFC"));
                tv.setTextSize(14f);
                return tv;
            }
        };
        listPcs.setAdapter(adapter);
        listPcs.setOnItemClickListener((parent, view, position, id) -> {
            LanDiscovery.DiscoveredPc pc = discovered.get(position);
            editPcIp.setText(pc.ip);
            txtStatus.setText("Selected: " + pc.hostname + " (" + pc.ip + ")");
        });

        btnScan.setOnClickListener(v -> startLanScan());
        btnToggle.setOnClickListener(v -> onToggleClicked());

        updateToggleButton(AudioCaptureService.isStreaming);
        startLanScan();
    }

    @Override
    protected void onResume() {
        super.onResume();
        IntentFilter filter = new IntentFilter(AudioCaptureService.ACTION_STATUS);
        if (Build.VERSION.SDK_INT >= 33) {
            registerReceiver(statusReceiver, filter, Context.RECEIVER_NOT_EXPORTED);
        } else {
            registerReceiver(statusReceiver, filter);
        }
        updateToggleButton(AudioCaptureService.isStreaming);
    }

    @Override
    protected void onPause() {
        try {
            unregisterReceiver(statusReceiver);
        } catch (Throwable ignored) {
        }
        super.onPause();
    }

    private void startLanScan() {
        btnScan.setEnabled(false);
        txtStatus.setText("Status: Scanning Wi-Fi LAN for AetherSora PC...");
        new Thread(() -> {
            List<LanDiscovery.DiscoveredPc> found = LanDiscovery.scan(1200);
            runOnUiThread(() -> {
                discovered.clear();
                discovered.addAll(found);
                adapter.notifyDataSetChanged();
                btnScan.setEnabled(true);
                if (!AudioCaptureService.isStreaming) {
                    if (!found.isEmpty()) {
                        LanDiscovery.DiscoveredPc first = found.get(0);
                        if (editPcIp.getText().toString().trim().isEmpty()) {
                            editPcIp.setText(first.ip);
                        }
                        txtStatus.setText("Found " + found.size() + " PC(s) on LAN — Ready to connect");
                    } else {
                        txtStatus.setText("No PC discovered yet. Ensure AetherSora PC is in Auto or Wi-Fi Companion mode.");
                    }
                }
            });
        }, "aether-scan").start();
    }

    private void onToggleClicked() {
        if (AudioCaptureService.isStreaming) {
            Intent stop = new Intent(this, AudioCaptureService.class);
            stop.setAction(AudioCaptureService.ACTION_STOP);
            startService(stop);
            return;
        }

        String ip = editPcIp.getText().toString().trim();
        if (ip.isEmpty()) {
            txtStatus.setText("Status: Enter your PC's Wi-Fi IP or tap Scan LAN first.");
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

    private void requestProjectionConsent() {
        MediaProjectionManager mpm =
                (MediaProjectionManager) getSystemService(Context.MEDIA_PROJECTION_SERVICE);
        startActivityForResult(mpm.createScreenCaptureIntent(), REQ_MEDIA_PROJECTION);
    }

    @Override
    public void onRequestPermissionsResult(int requestCode, String[] permissions, int[] grantResults) {
        if (requestCode == REQ_RECORD_AUDIO
                && grantResults.length > 0
                && grantResults[0] == PackageManager.PERMISSION_GRANTED) {
            requestProjectionConsent();
        } else {
            txtStatus.setText("Status: Audio permission is required for AudioPlaybackCapture.");
        }
    }

    @Override
    protected void onActivityResult(int requestCode, int resultCode, Intent data) {
        if (requestCode == REQ_MEDIA_PROJECTION) {
            if (resultCode == RESULT_OK && data != null) {
                String ip = editPcIp.getText().toString().trim();
                Intent svc = new Intent(this, AudioCaptureService.class);
                svc.setAction(AudioCaptureService.ACTION_START);
                svc.putExtra(AudioCaptureService.EXTRA_RESULT_CODE, resultCode);
                svc.putExtra(AudioCaptureService.EXTRA_RESULT_DATA, data);
                svc.putExtra(AudioCaptureService.EXTRA_PC_IP, ip);
                svc.putExtra(AudioCaptureService.EXTRA_PC_PORT, 19480);
                svc.putExtra(AudioCaptureService.EXTRA_AUTO_MUTE, chkAutoMute.isChecked());
                startForegroundService(svc);
                updateToggleButton(true);
            } else {
                txtStatus.setText("Status: Capture prompt canceled.");
            }
        }
    }

    private void updateToggleButton(boolean streaming) {
        if (streaming) {
            btnToggle.setText("Disconnect");
            btnToggle.setBackgroundColor(Color.parseColor("#FACC15"));
        } else {
            btnToggle.setText("Connect & Stream Lossless");
            btnToggle.setBackgroundColor(Color.parseColor("#38BDF8"));
        }
    }
}

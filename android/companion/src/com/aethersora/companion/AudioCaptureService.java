package com.aethersora.companion;

import android.app.Notification;
import android.app.NotificationChannel;
import android.app.NotificationManager;
import android.app.PendingIntent;
import android.app.Service;
import android.content.Context;
import android.content.Intent;
import android.content.pm.ServiceInfo;
import android.media.AudioAttributes;
import android.media.AudioFormat;
import android.media.AudioManager;
import android.media.AudioPlaybackCaptureConfiguration;
import android.media.AudioRecord;
import android.media.projection.MediaProjection;
import android.media.projection.MediaProjectionManager;
import android.os.Build;
import android.os.IBinder;

import java.net.DatagramPacket;
import java.net.DatagramSocket;
import java.net.InetAddress;
import java.nio.ByteBuffer;
import java.nio.ByteOrder;
import java.util.concurrent.atomic.AtomicBoolean;

/**
 * Foreground MediaProjection service capturing Android 10+ internal audio via
 * AudioPlaybackCaptureConfiguration (No Developer Mode / No ADB) and streaming
 * 5 ms 48 kHz 16-bit Stereo PCM UDP packets to AetherSora PC.
 */
public final class AudioCaptureService extends Service {
    public static final String ACTION_START = "com.aethersora.companion.START";
    public static final String ACTION_STOP = "com.aethersora.companion.STOP";
    public static final String ACTION_STATUS = "com.aethersora.companion.STATUS";

    public static final String EXTRA_RESULT_CODE = "result_code";
    public static final String EXTRA_RESULT_DATA = "result_data";
    public static final String EXTRA_PC_IP = "pc_ip";
    public static final String EXTRA_PC_PORT = "pc_port";
    public static final String EXTRA_AUTO_MUTE = "auto_mute";
    public static final String EXTRA_RUNNING = "running";
    public static final String EXTRA_MESSAGE = "message";

    private static final String CHANNEL_ID = "aethersora_stream";
    private static final int NOTIF_ID = 1048;

    private static final int SAMPLE_RATE = 48000;
    private static final int CHANNELS = 2;
    private static final int FRAMES_PER_PACKET = 240;
    private static final int PCM_BYTES_PER_PACKET = FRAMES_PER_PACKET * CHANNELS * 2;
    private static final int HEADER_BYTES = 12;
    private static final int PACKET_BYTES = HEADER_BYTES + PCM_BYTES_PER_PACKET;

    private static final short MAGIC = (short) 0xA55A;
    private static final byte FLAG_AUDIO = 0x00;
    private static final byte FLAG_KEEPALIVE = 0x01;
    private static final byte FLAG_HELLO = 0x02;
    private static final byte FLAG_BYE = 0x04;

    public static volatile boolean isStreaming = false;

    private final AtomicBoolean running = new AtomicBoolean(false);
    private MediaProjection mediaProjection;
    private Thread captureThread;
    private int savedMusicVolume = -1;
    private boolean didAutoMute = false;

    @Override
    public IBinder onBind(Intent intent) {
        return null;
    }

    @Override
    public int onStartCommand(Intent intent, int flags, int startId) {
        if (intent == null || ACTION_STOP.equals(intent.getAction())) {
            stopStreamingAndSelf();
            return START_NOT_STICKY;
        }

        if (ACTION_START.equals(intent.getAction())) {
            final int resultCode = intent.getIntExtra(EXTRA_RESULT_CODE, 0);
            final Intent resultData = intent.getParcelableExtra(EXTRA_RESULT_DATA);
            final String pcIp = intent.getStringExtra(EXTRA_PC_IP);
            final int pcPort = intent.getIntExtra(EXTRA_PC_PORT, 19480);
            final boolean autoMute = intent.getBooleanExtra(EXTRA_AUTO_MUTE, true);

            if (resultData == null || pcIp == null || pcIp.trim().isEmpty()) {
                broadcastStatus(false, "Status: Missing capture permission or PC IP");
                stopSelf();
                return START_NOT_STICKY;
            }

            startForegroundNotification(pcIp, pcPort);

            MediaProjectionManager mpm =
                    (MediaProjectionManager) getSystemService(Context.MEDIA_PROJECTION_SERVICE);
            mediaProjection = mpm.getMediaProjection(resultCode, resultData);
            if (mediaProjection == null) {
                broadcastStatus(false, "Status: MediaProjection denied");
                stopSelf();
                return START_NOT_STICKY;
            }

            running.set(true);
            isStreaming = true;
            captureThread = new Thread(() -> runCaptureLoop(pcIp.trim(), pcPort, autoMute), "aether-capture");
            captureThread.start();
        }

        return START_NOT_STICKY;
    }

    private void startForegroundNotification(String pcIp, int pcPort) {
        NotificationManager nm = (NotificationManager) getSystemService(Context.NOTIFICATION_SERVICE);
        NotificationChannel ch = new NotificationChannel(
                CHANNEL_ID, "AetherSora Audio Stream", NotificationManager.IMPORTANCE_LOW);
        nm.createNotificationChannel(ch);

        Intent stopIntent = new Intent(this, AudioCaptureService.class);
        stopIntent.setAction(ACTION_STOP);
        PendingIntent stopPi = PendingIntent.getService(
                this, 0, stopIntent, PendingIntent.FLAG_IMMUTABLE);

        Notification notif = new Notification.Builder(this, CHANNEL_ID)
                .setSmallIcon(R.drawable.ic_launcher)
                .setContentTitle("AetherSora Streaming (48 kHz Lossless)")
                .setContentText("Sending PCM audio to " + pcIp + ":" + pcPort)
                .addAction(new Notification.Action.Builder(null, "Disconnect", stopPi).build())
                .setOngoing(true)
                .build();

        if (Build.VERSION.SDK_INT >= 29) {
            startForeground(NOTIF_ID, notif, ServiceInfo.FOREGROUND_SERVICE_TYPE_MEDIA_PROJECTION);
        } else {
            startForeground(NOTIF_ID, notif);
        }
    }

    private void runCaptureLoop(String pcIp, int pcPort, boolean autoMute) {
        AudioRecord recorder = null;
        DatagramSocket socket = null;
        InetAddress pcAddr = null;
        byte[] packetBuf = new byte[PACKET_BYTES];
        ByteBuffer headerView = ByteBuffer.wrap(packetBuf, 0, HEADER_BYTES).order(ByteOrder.LITTLE_ENDIAN);

        try {
            AudioPlaybackCaptureConfiguration config =
                    new AudioPlaybackCaptureConfiguration.Builder(mediaProjection)
                            .addMatchingUsage(AudioAttributes.USAGE_MEDIA)
                            .addMatchingUsage(AudioAttributes.USAGE_GAME)
                            .addMatchingUsage(AudioAttributes.USAGE_UNKNOWN)
                            .build();

            AudioFormat audioFormat = new AudioFormat.Builder()
                    .setEncoding(AudioFormat.ENCODING_PCM_16BIT)
                    .setSampleRate(SAMPLE_RATE)
                    .setChannelMask(AudioFormat.CHANNEL_IN_STEREO)
                    .build();

            int minBuf = AudioRecord.getMinBufferSize(
                    SAMPLE_RATE, AudioFormat.CHANNEL_IN_STEREO, AudioFormat.ENCODING_PCM_16BIT);
            int bufSize = Math.max(minBuf * 2, PCM_BYTES_PER_PACKET * 8);

            recorder = new AudioRecord.Builder()
                    .setAudioFormat(audioFormat)
                    .setBufferSizeInBytes(bufSize)
                    .setAudioPlaybackCaptureConfig(config)
                    .build();

            pcAddr = InetAddress.getByName(pcIp);
            socket = new DatagramSocket();
            socket.setTrafficClass(0xB8);
            socket.setSoTimeout(500);

            writeHeader(headerView, 0, FLAG_HELLO);
            socket.send(new DatagramPacket(packetBuf, HEADER_BYTES, pcAddr, pcPort));
            boolean acked = false;
            try {
                byte[] ackBuf = new byte[16];
                DatagramPacket ackPkt = new DatagramPacket(ackBuf, ackBuf.length);
                socket.receive(ackPkt);
                acked = ackPkt.getLength() >= 4 && ackBuf[0] == 0x5A && ackBuf[1] == (byte) 0xA5;
            } catch (Throwable ignored) {
            }

            broadcastStatus(true, acked
                    ? "Status: Connected & Streaming 48 kHz Lossless -> " + pcIp
                    : "Status: Streaming 48 kHz PCM -> " + pcIp + " (unacked; check PC Firewall)");

            recorder.startRecording();
            applyAutoMuteIfEnabled(autoMute);
            DatagramPacket audioPacket = new DatagramPacket(packetBuf, PACKET_BYTES, pcAddr, pcPort);
            DatagramPacket keepalivePacket = new DatagramPacket(packetBuf, HEADER_BYTES, pcAddr, pcPort);

            int seq = 1;
            int silentCount = 0;
            long lastKeepaliveNs = System.nanoTime();

            while (running.get()) {
                int readTotal = 0;
                while (readTotal < PCM_BYTES_PER_PACKET && running.get()) {
                    int r = recorder.read(
                            packetBuf, HEADER_BYTES + readTotal, PCM_BYTES_PER_PACKET - readTotal);
                    if (r <= 0) {
                        break;
                    }
                    readTotal += r;
                }
                if (readTotal < PCM_BYTES_PER_PACKET) {
                    break;
                }

                boolean silent = isSilent(packetBuf, HEADER_BYTES, PCM_BYTES_PER_PACKET);
                silentCount = silent ? (silentCount + 1) : 0;

                if (silentCount <= 50) {
                    writeHeader(headerView, seq++, FLAG_AUDIO);
                    socket.send(audioPacket);
                    lastKeepaliveNs = System.nanoTime();
                } else {
                    long now = System.nanoTime();
                    if (now - lastKeepaliveNs >= 400_000_000L) {
                        writeHeader(headerView, seq, FLAG_KEEPALIVE);
                        socket.send(keepalivePacket);
                        lastKeepaliveNs = now;
                    }
                }
            }
        } catch (Throwable t) {
            broadcastStatus(false, "Status: Error — " + t.getMessage());
        } finally {
            if (socket != null && pcAddr != null) {
                try {
                    writeHeader(headerView, 0, FLAG_BYE);
                    socket.send(new DatagramPacket(packetBuf, HEADER_BYTES, pcAddr, pcPort));
                } catch (Throwable ignored) {
                }
                socket.close();
            }
            if (recorder != null) {
                try {
                    recorder.stop();
                } catch (Throwable ignored) {
                }
                try {
                    recorder.release();
                } catch (Throwable ignored) {
                }
            }
            stopStreamingAndSelf();
        }
    }

    private synchronized void applyAutoMuteIfEnabled(boolean autoMute) {
        if (!autoMute || didAutoMute) {
            return;
        }
        try {
            AudioManager am = (AudioManager) getSystemService(Context.AUDIO_SERVICE);
            if (am != null) {
                int current = am.getStreamVolume(AudioManager.STREAM_MUSIC);
                if (current > 0) {
                    savedMusicVolume = current;
                    am.setStreamVolume(AudioManager.STREAM_MUSIC, 0, 0);
                    didAutoMute = true;
                }
            }
        } catch (Throwable ignored) {
        }
    }

    private synchronized void restorePhoneVolumeIfMuted() {
        if (!didAutoMute) {
            return;
        }
        didAutoMute = false;
        try {
            AudioManager am = (AudioManager) getSystemService(Context.AUDIO_SERVICE);
            if (am != null && savedMusicVolume > 0) {
                if (am.getStreamVolume(AudioManager.STREAM_MUSIC) == 0) {
                    am.setStreamVolume(AudioManager.STREAM_MUSIC, savedMusicVolume, 0);
                }
            }
        } catch (Throwable ignored) {
        }
        savedMusicVolume = -1;
    }

    private void stopStreamingAndSelf() {
        running.set(false);
        isStreaming = false;
        restorePhoneVolumeIfMuted();
        if (mediaProjection != null) {
            try {
                mediaProjection.stop();
            } catch (Throwable ignored) {
            }
            mediaProjection = null;
        }
        broadcastStatus(false, "Status: Disconnected");
        stopForeground(STOP_FOREGROUND_REMOVE);
        stopSelf();
    }

    private void broadcastStatus(boolean active, String message) {
        Intent i = new Intent(ACTION_STATUS);
        i.setPackage(getPackageName());
        i.putExtra(EXTRA_RUNNING, active);
        i.putExtra(EXTRA_MESSAGE, message);
        sendBroadcast(i);
    }

    private static void writeHeader(ByteBuffer view, int seq, byte flags) {
        view.clear();
        view.putShort(MAGIC);
        view.putInt(seq);
        view.putInt(SAMPLE_RATE);
        view.put((byte) CHANNELS);
        view.put(flags);
    }

    private static boolean isSilent(byte[] buf, int offset, int len) {
        int end = offset + len;
        for (int i = offset; i < end; i++) {
            if (buf[i] != 0) {
                return false;
            }
        }
        return true;
    }

    @Override
    public void onTaskRemoved(Intent rootIntent) {
        stopStreamingAndSelf();
        super.onTaskRemoved(rootIntent);
    }

    @Override
    public void onDestroy() {
        running.set(false);
        isStreaming = false;
        restorePhoneVolumeIfMuted();
        if (captureThread != null) {
            captureThread.interrupt();
        }
        super.onDestroy();
    }
}

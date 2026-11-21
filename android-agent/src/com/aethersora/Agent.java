package com.aethersora;

import android.media.AudioFormat;
import android.media.AudioRecord;
import android.media.MediaRecorder;
import java.io.InputStream;
import java.net.DatagramPacket;
import java.net.DatagramSocket;
import java.net.InetAddress;
import java.nio.ByteBuffer;
import java.nio.ByteOrder;
import java.util.concurrent.atomic.AtomicBoolean;

/**
 * Ultra-lightweight Android audio capture agent executed via `adb shell app_process` (UID 2000).
 * Captures 48 kHz 16-bit Stereo PCM from REMOTE_SUBMIX (source 8) and streams 5 ms UDP packets
 * directly over 5 GHz Wi-Fi LAN to the Windows PC receiver.
 *
 * Exits immediately and releases REMOTE_SUBMIX (unmuting phone speaker) when stdin closes.
 */
public final class Agent {
    private static final int SAMPLE_RATE = 48000;
    private static final int CHANNELS = 2;
    private static final int BYTES_PER_FRAME = CHANNELS * 2; // 16-bit stereo = 4 bytes
    private static final int FRAMES_PER_PACKET = 240;        // 5 ms at 48 kHz
    private static final int PCM_BYTES_PER_PACKET = FRAMES_PER_PACKET * BYTES_PER_FRAME; // 960 bytes
    private static final int HEADER_BYTES = 12;
    private static final int PACKET_BYTES = HEADER_BYTES + PCM_BYTES_PER_PACKET; // 972 bytes (< MTU)

    private static final short MAGIC = (short) 0xA55A;
    private static final byte FLAG_AUDIO = 0x00;
    private static final byte FLAG_KEEPALIVE = 0x01;

    // MediaRecorder.AudioSource.REMOTE_SUBMIX = 8
    private static final int AUDIO_SOURCE_REMOTE_SUBMIX = 8;

    public static void main(String[] args) {
        if (args.length < 2) {
            System.out.println("ERR:USAGE Agent <pc_ip> <udp_port>");
            System.out.flush();
            System.exit(2);
            return;
        }

        final String targetIp = args[0];
        final int targetPort;
        try {
            targetPort = Integer.parseInt(args[1]);
        } catch (NumberFormatException e) {
            System.out.println("ERR:INVALID_PORT");
            System.out.flush();
            System.exit(2);
            return;
        }

        final AtomicBoolean running = new AtomicBoolean(true);

        // Lifeline thread: as soon as the PC closes stdin or ADB drops, exit cleanly
        // so AudioRecord is released and the phone's internal speaker is unmuted.
        Thread lifeline = new Thread(new Runnable() {
            @Override
            public void run() {
                try {
                    InputStream in = System.in;
                    byte[] buf = new byte[16];
                    while (running.get()) {
                        int n = in.read(buf);
                        if (n < 0) {
                            break;
                        }
                    }
                } catch (Throwable ignored) {
                } finally {
                    running.set(false);
                }
            }
        }, "aether-lifeline");
        lifeline.setDaemon(true);
        lifeline.start();

        int minBuf = AudioRecord.getMinBufferSize(
                SAMPLE_RATE,
                AudioFormat.CHANNEL_IN_STEREO,
                AudioFormat.ENCODING_PCM_16BIT
        );
        if (minBuf <= 0) {
            minBuf = SAMPLE_RATE * BYTES_PER_FRAME / 10;
        }
        int recordBufSize = Math.max(minBuf * 2, PCM_BYTES_PER_PACKET * 8);

        AudioRecord recorder = null;
        DatagramSocket socket = null;

        try {
            recorder = createSubmixRecorder(recordBufSize);
            if (recorder == null || recorder.getState() != AudioRecord.STATE_INITIALIZED) {
                System.out.println("ERR:AUDIO_INIT_FAILED");
                System.out.flush();
                System.exit(3);
                return;
            }

            InetAddress pcAddress = InetAddress.getByName(targetIp);
            socket = new DatagramSocket();
            socket.setTrafficClass(0xB8); // DSCP EF (Expedited Forwarding) for low-latency Wi-Fi WMM VO/VI

            recorder.startRecording();
            if (recorder.getRecordingState() != AudioRecord.RECORDSTATE_RECORDING) {
                System.out.println("ERR:AUDIO_START_FAILED");
                System.out.flush();
                System.exit(4);
                return;
            }

            System.out.println("READY");
            System.out.flush();

            byte[] packetBuf = new byte[PACKET_BYTES];
            ByteBuffer headerView = ByteBuffer.wrap(packetBuf, 0, HEADER_BYTES).order(ByteOrder.LITTLE_ENDIAN);
            DatagramPacket audioPacket = new DatagramPacket(packetBuf, PACKET_BYTES, pcAddress, targetPort);
            DatagramPacket keepalivePacket = new DatagramPacket(packetBuf, HEADER_BYTES, pcAddress, targetPort);

            int seq = 0;
            int silentFramesCount = 0;
            long lastKeepaliveNs = System.nanoTime();

            while (running.get()) {
                int readTotal = 0;
                while (readTotal < PCM_BYTES_PER_PACKET && running.get()) {
                    int r = recorder.read(
                            packetBuf,
                            HEADER_BYTES + readTotal,
                            PCM_BYTES_PER_PACKET - readTotal
                    );
                    if (r <= 0) {
                        break;
                    }
                    readTotal += r;
                }

                if (readTotal < PCM_BYTES_PER_PACKET) {
                    break;
                }

                boolean isSilent = isBufferSilent(packetBuf, HEADER_BYTES, PCM_BYTES_PER_PACKET);
                if (isSilent) {
                    silentFramesCount++;
                } else {
                    silentFramesCount = 0;
                }

                // Keep streaming audio for 50 packets (250 ms) after silence starts to preserve decay tails.
                // After 250 ms of pure silence, switch to lightweight 12-byte keepalives every 400 ms.
                if (silentFramesCount <= 50) {
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
            System.out.println("ERR:EXCEPTION:" + t.getClass().getSimpleName() + ":" + t.getMessage());
            System.out.flush();
        } finally {
            running.set(false);
            if (socket != null) {
                try {
                    socket.close();
                } catch (Throwable ignored) {
                }
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
        }
    }

    @SuppressWarnings("deprecation")
    private static AudioRecord createSubmixRecorder(int bufferSize) {
        try {
            return new AudioRecord(
                    AUDIO_SOURCE_REMOTE_SUBMIX,
                    SAMPLE_RATE,
                    AudioFormat.CHANNEL_IN_STEREO,
                    AudioFormat.ENCODING_PCM_16BIT,
                    bufferSize
            );
        } catch (Throwable t) {
            return null;
        }
    }

    private static void writeHeader(ByteBuffer view, int seq, byte flags) {
        view.clear();
        view.putShort(MAGIC);
        view.putInt(seq);
        view.putInt(SAMPLE_RATE);
        view.put((byte) CHANNELS);
        view.put(flags);
    }

    private static boolean isBufferSilent(byte[] buf, int offset, int length) {
        int end = offset + length;
        for (int i = offset; i < end; i++) {
            if (buf[i] != 0) {
                return false;
            }
        }
        return true;
    }
}

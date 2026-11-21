package com.aethersora.companion;

import java.net.DatagramPacket;
import java.net.DatagramSocket;
import java.net.InetAddress;
import java.net.InterfaceAddress;
import java.net.NetworkInterface;
import java.nio.charset.StandardCharsets;
import java.util.ArrayList;
import java.util.Enumeration;
import java.util.LinkedHashMap;
import java.util.List;
import java.util.Map;

/**
 * Discovers running AetherSora PC receivers on the local Wi-Fi LAN via UDP port 19481.
 */
public final class LanDiscovery {
    public static final int DISCOVERY_PORT = 19481;
    private static final byte[] DISCOVER_MAGIC =
            "AETHERSORA_DISCOVER_V1".getBytes(StandardCharsets.UTF_8);
    private static final String REPLY_PREFIX = "AETHERSORA_PC_V1|";

    public static final class DiscoveredPc {
        public final String hostname;
        public final String ip;
        public final int audioPort;

        public DiscoveredPc(String hostname, String ip, int audioPort) {
            this.hostname = hostname;
            this.ip = ip;
            this.audioPort = audioPort;
        }

        @Override
        public String toString() {
            return hostname + "  (" + ip + ":" + audioPort + ")";
        }
    }

    public static List<DiscoveredPc> scan(int timeoutMs) {
        Map<String, DiscoveredPc> found = new LinkedHashMap<>();
        DatagramSocket socket = null;
        try {
            socket = new DatagramSocket();
            socket.setBroadcast(true);
            socket.setSoTimeout(250);

            sendBroadcast(socket, InetAddress.getByName("255.255.255.255"));

            Enumeration<NetworkInterface> ifaces = NetworkInterface.getNetworkInterfaces();
            while (ifaces != null && ifaces.hasMoreElements()) {
                NetworkInterface iface = ifaces.nextElement();
                if (!iface.isUp() || iface.isLoopback()) {
                    continue;
                }
                for (InterfaceAddress ia : iface.getInterfaceAddresses()) {
                    InetAddress bcast = ia.getBroadcast();
                    if (bcast != null) {
                        sendBroadcast(socket, bcast);
                    }
                }
            }

            long deadline = System.currentTimeMillis() + timeoutMs;
            byte[] buf = new byte[256];
            while (System.currentTimeMillis() < deadline) {
                try {
                    DatagramPacket pkt = new DatagramPacket(buf, buf.length);
                    socket.receive(pkt);
                    String msg = new String(pkt.getData(), 0, pkt.getLength(), StandardCharsets.UTF_8).trim();
                    if (msg.startsWith(REPLY_PREFIX)) {
                        String[] parts = msg.split("\\|");
                        String host = parts.length >= 2 ? parts[1] : "Windows PC";
                        int port = 19480;
                        if (parts.length >= 3) {
                            try {
                                port = Integer.parseInt(parts[2]);
                            } catch (NumberFormatException ignored) {
                            }
                        }
                        String ip = pkt.getAddress().getHostAddress();
                        if (ip != null) {
                            found.put(ip, new DiscoveredPc(host, ip, port));
                        }
                    }
                } catch (Throwable ignored) {
                }
            }
        } catch (Throwable ignored) {
        } finally {
            if (socket != null) {
                socket.close();
            }
        }
        return new ArrayList<>(found.values());
    }

    private static void sendBroadcast(DatagramSocket socket, InetAddress target) {
        try {
            DatagramPacket pkt = new DatagramPacket(
                    DISCOVER_MAGIC, DISCOVER_MAGIC.length, target, DISCOVERY_PORT);
            socket.send(pkt);
        } catch (Throwable ignored) {
        }
    }
}

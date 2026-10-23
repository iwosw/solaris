package org.solaris.bukkit;

import java.nio.charset.StandardCharsets;
import org.teavm.interop.Import;

/** Primitive ABI keeps Java and the WIT adapter's independent memories separate. */
public final class Bridge {
    private Bridge() {}
    @Import(module = "solaris", name = "output_byte")
    private static native void outputByte(int value);
    @Import(module = "solaris", name = "finish_output")
    private static native void finish(int kind, long session);
    @Import(module = "solaris", name = "input_length")
    private static native int length(int field);
    @Import(module = "solaris", name = "input_byte")
    private static native int inputByte(int field, int offset);
    public static String input(int field) {
        byte[] bytes = new byte[length(field)];
        for (int i = 0; i < bytes.length; i++) bytes[i] = (byte) inputByte(field, i);
        return new String(bytes, StandardCharsets.UTF_8);
    }
    private static void output(String message, int kind, long session) {
        byte[] bytes = message.getBytes(StandardCharsets.UTF_8);
        if (bytes.length > 8192) throw new IllegalArgumentException("message exceeds 8192 bytes");
        for (byte value : bytes) outputByte(value & 255);
        finish(kind, session);
    }
    public static void log(String message) { output(message, 0, 0); }
    public static void message(long session, String message) { output(message, 1, session); }
}

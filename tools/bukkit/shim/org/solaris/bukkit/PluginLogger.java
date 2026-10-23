package org.solaris.bukkit;

/** Offline replacement for the supported java.util.logging.Logger operations. */
public final class PluginLogger {
    private final String name;

    private PluginLogger(String name) {
        this.name = name;
    }

    public static PluginLogger getLogger(String name) {
        return new PluginLogger(name);
    }

    public String getName() {
        return name;
    }

    public void info(String message) {
        Bridge.log(message);
    }
}

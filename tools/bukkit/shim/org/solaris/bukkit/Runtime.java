package org.solaris.bukkit;
import java.util.ArrayList;
import java.util.LinkedHashMap;
import org.bukkit.Server;
import org.bukkit.command.PluginCommand;
import org.bukkit.event.Listener;
import org.bukkit.plugin.PluginManager;
import org.bukkit.plugin.java.JavaPlugin;
public final class Runtime {
    private Runtime() {}
    public static JavaPlugin plugin;
    public static final ArrayList<Listener> listeners = new ArrayList<>();
    public static final LinkedHashMap<String, PluginCommand> commands = new LinkedHashMap<>();
    private static final PluginManager MANAGER = (listener, owner) -> {
        if (owner != plugin) throw new IllegalArgumentException("foreign plugin");
        if (listeners.size() >= 128) throw new IllegalStateException("listener limit");
        if (!listeners.contains(listener)) listeners.add(listener);
    };
    public static final Server SERVER = () -> MANAGER;
}

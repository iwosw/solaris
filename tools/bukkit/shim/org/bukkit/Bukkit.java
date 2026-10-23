package org.bukkit;
import org.bukkit.plugin.PluginManager;
import org.solaris.bukkit.Runtime;
public final class Bukkit {
    private Bukkit() {}
    public static Server getServer() { return Runtime.SERVER; }
    public static PluginManager getPluginManager() { return Runtime.SERVER.getPluginManager(); }
}

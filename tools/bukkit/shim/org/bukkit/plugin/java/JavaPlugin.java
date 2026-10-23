package org.bukkit.plugin.java;

import org.bukkit.Server;
import org.bukkit.command.*;
import org.bukkit.plugin.Plugin;
import org.solaris.bukkit.PluginLogger;
import org.solaris.bukkit.Runtime;

public class JavaPlugin implements Plugin, CommandExecutor {
    public PluginLogger getLogger() { return PluginLogger.getLogger("Bukkit"); }
    public Server getServer() { return Runtime.SERVER; }
    public PluginCommand getCommand(String name) { return Runtime.commands.get(name); }
    public void onEnable() {}
    public void onDisable() {}
    public boolean onCommand(CommandSender sender, Command command, String label, String[] args) {
        return false;
    }
}

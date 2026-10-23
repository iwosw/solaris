package example;

import org.bukkit.Material;
import org.bukkit.command.Command;
import org.bukkit.command.CommandSender;
import org.bukkit.event.EventHandler;
import org.bukkit.event.Listener;
import org.bukkit.event.block.BlockBreakEvent;
import org.bukkit.event.player.PlayerJoinEvent;
import org.bukkit.plugin.java.JavaPlugin;

/** Compiled against the upstream Spigot API JAR, not Solaris' replacement types. */
public final class HelloPlugin extends JavaPlugin implements Listener {
    @Override public void onEnable() {
        getLogger().info("BUKKIT_ON_ENABLE");
        getServer().getPluginManager().registerEvents(this, this);
        getCommand("hello").setExecutor(this);
    }
    @Override public boolean onCommand(CommandSender sender, Command command, String label, String[] args) {
        sender.sendMessage("Hello, " + sender.getName() + "! " + String.join(" ", args));
        return true;
    }
    @EventHandler public void joined(PlayerJoinEvent event) {
        event.getPlayer().sendMessage("Welcome, " + event.getPlayer().getName() + "!");
    }
    @EventHandler public void breaking(BlockBreakEvent event) {
        if (event.getBlock().getType() == Material.DIAMOND_ORE) event.setCancelled(true);
    }
    @Override public void onDisable() { getLogger().info("BUKKIT_ON_DISABLE"); }
}

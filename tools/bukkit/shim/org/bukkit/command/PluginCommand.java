package org.bukkit.command;
public final class PluginCommand extends Command {
    private CommandExecutor executor;
    public PluginCommand(String name, CommandExecutor executor) { super(name); this.executor = executor; }
    public void setExecutor(CommandExecutor executor) { this.executor = executor; }
    public boolean execute(CommandSender sender, String label, String[] args) {
        return executor.onCommand(sender, this, label, args);
    }
}

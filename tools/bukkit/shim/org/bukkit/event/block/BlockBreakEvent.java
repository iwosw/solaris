package org.bukkit.event.block;
import org.bukkit.block.Block;
import org.bukkit.entity.Player;
import org.bukkit.event.Event;
import org.bukkit.event.Cancellable;
public class BlockBreakEvent extends Event implements Cancellable {
    private final Block block;
    private final Player player;
    private boolean cancelled;
    public BlockBreakEvent(Block block, Player player) { this.block = block; this.player = player; }
    public Block getBlock() { return block; }
    public Player getPlayer() { return player; }
    public boolean isCancelled() { return cancelled; }
    public void setCancelled(boolean value) { cancelled = value; }
}

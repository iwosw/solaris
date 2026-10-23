package org.solaris.bukkit;
import org.bukkit.Material;
import org.bukkit.block.Block;
public final class SnapshotBlock implements Block {
    private final Material material;
    private final int x, y, z;
    public SnapshotBlock(Material material, int x, int y, int z) {
        this.material = material; this.x = x; this.y = y; this.z = z;
    }
    public Material getType() { return material; }
    public int getX() { return x; }
    public int getY() { return y; }
    public int getZ() { return z; }
}

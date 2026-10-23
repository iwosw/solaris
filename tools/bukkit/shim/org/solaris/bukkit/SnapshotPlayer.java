package org.solaris.bukkit;
import org.bukkit.entity.Player;
import java.util.UUID;
public final class SnapshotPlayer implements Player {
    private final long session;
    private final String uuid;
    private final String name;
    public SnapshotPlayer(long session, String uuid, String name) {
        this.session = session; this.uuid = uuid; this.name = name;
    }
    public void sendMessage(String message) { Bridge.message(session, message); }
    public String getName() {
        if (name == null) throw new UnsupportedOperationException("precommit has no player name");
        return name;
    }
    public UUID getUniqueId() { return UUID.fromString(uuid); }
}

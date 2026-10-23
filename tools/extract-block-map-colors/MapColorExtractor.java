import com.google.gson.stream.JsonWriter;
import java.io.FileWriter;
import net.minecraft.SharedConstants;
import net.minecraft.core.BlockPos;
import net.minecraft.server.Bootstrap;
import net.minecraft.world.level.EmptyBlockGetter;
import net.minecraft.world.level.block.Block;

/** Extract registry facts, not Mojang implementation source. */
public final class MapColorExtractor {
    public static void main(String[] args) throws Exception {
        SharedConstants.tryDetectVersion();
        Bootstrap.bootStrap();
        try (JsonWriter out = new JsonWriter(new FileWriter(args[0]))) {
            out.beginArray();
            for (var state : Block.BLOCK_STATE_REGISTRY) {
                out.value(state.getMapColor(EmptyBlockGetter.INSTANCE, BlockPos.ZERO).id);
            }
            out.endArray();
        }
    }
}

// Iterates BuiltInRegistries.BLOCK against the unobfuscated vanilla
// 26.1.x server classes and writes per-block-state light metadata to
// a JSON file. Per ADR 0001 we treat the resulting JSON as data, not
// code: identifiers, field semantics, and the iteration come from
// public Mojang APIs (BlockBehaviour$BlockStateBase.getLightEmission,
// getLightDampening, propagatesSkylightDown), the same posture as
// vanilla's `--reports` data generator.
//
// Output JSON shape:
//   {
//     "version": "<vanilla version id, e.g. 26.1.2>",
//     "max_state_id": <largest BlockState id seen>,
//     "entries": [
//        [emission, dampening, propagates_sky_0_or_1, suffocating_0_or_1],
//        ...    // index = global state-id; states with no Block
//        ...    // entry in the registry get [0, 0, 1] (sentinel).
//     ]
//   }

import com.google.gson.JsonElement;
import com.google.gson.JsonParser;
import com.google.gson.stream.JsonWriter;

import java.io.File;
import java.io.FileWriter;
import java.io.IOException;
import java.io.InputStreamReader;
import java.lang.reflect.Method;
import java.nio.charset.StandardCharsets;
import java.util.ArrayList;
import java.util.Arrays;
import java.util.Collection;
import java.util.HashMap;
import java.util.HashSet;
import java.util.List;
import java.util.Map;
import java.util.Set;
import java.util.zip.ZipEntry;
import java.util.zip.ZipFile;

import net.minecraft.SharedConstants;
import net.minecraft.core.BlockPos;
import net.minecraft.core.Holder;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.core.registries.Registries;
import net.minecraft.resources.Identifier;
import net.minecraft.server.Bootstrap;
import net.minecraft.tags.TagKey;
import net.minecraft.world.level.EmptyBlockGetter;
import net.minecraft.world.level.block.Block;
import net.minecraft.world.level.block.state.BlockState;

public final class LightExtractor {
    private LightExtractor() {}

    public static void main(String[] args) throws IOException {
        if (args.length != 1) {
            System.err.println("usage: java LightExtractor <output.json>");
            System.exit(2);
        }

        SharedConstants.tryDetectVersion();
        Bootstrap.bootStrap();
        // 26.3's default isSuffocating is `is(#causes_suffocation) &&
        // isCollisionShapeFullBlock`; a bare bootstrap has no tags bound, so
        // stone would read as non-suffocating. Bind the bundle's own block tags.
        bindBundleBlockTags();

        int maxId = -1;
        for (Block block : BuiltInRegistries.BLOCK) {
            for (BlockState state : block.getStateDefinition().getPossibleStates()) {
                int id = Block.BLOCK_STATE_REGISTRY.getId(state);
                if (id > maxId) maxId = id;
            }
        }
        if (maxId < 0) {
            System.err.println("error: registry yielded no states");
            System.exit(1);
        }

        int[][] entries = new int[maxId + 1][];
        for (Block block : BuiltInRegistries.BLOCK) {
            for (BlockState state : block.getStateDefinition().getPossibleStates()) {
                int id = Block.BLOCK_STATE_REGISTRY.getId(state);
                int emission = state.getLightEmission();
                int dampening = state.getLightDampening();
                boolean propagates = state.propagatesSkylightDown();
                boolean suffocating = state.isSuffocating(EmptyBlockGetter.INSTANCE, BlockPos.ZERO);
                entries[id] = new int[] {
                    emission,
                    dampening,
                    propagates ? 1 : 0,
                    suffocating ? 1 : 0
                };
            }
        }

        int gaps = 0;
        for (int i = 0; i <= maxId; i++) {
            if (entries[i] == null) {
                gaps++;
                entries[i] = new int[] { 0, 0, 1, 0 };
            }
        }

        try (JsonWriter w = new JsonWriter(new FileWriter(args[0]))) {
            w.setIndent("");
            w.beginObject();
            w.name("version").value(SharedConstants.getCurrentVersion().name());
            w.name("max_state_id").value(maxId);
            w.name("entries").beginArray();
            for (int[] e : entries) {
                w.beginArray();
                w.value(e[0]);
                w.value(e[1]);
                w.value(e[2]);
                w.value(e[3]);
                w.endArray();
            }
            w.endArray();
            w.endObject();
        }

        System.err.printf("wrote %d entries (max_state_id=%d, gaps=%d)%n",
            entries.length, maxId, gaps);
    }

    private static final String BLOCK_TAG_DIR = "data/minecraft/tags/block/";

    /** Bind every `data/minecraft/tags/block` tag of the server jar to its blocks. */
    private static void bindBundleBlockTags() throws IOException {
        Map<String, List<JsonElement>> raw = new HashMap<>();
        File jar;
        try {
            jar = new File(Block.class.getProtectionDomain().getCodeSource().getLocation().toURI());
        } catch (java.net.URISyntaxException e) {
            throw new IOException("locate the server jar", e);
        }
        try (ZipFile zip = new ZipFile(jar)) {
            var entries = zip.entries();
            while (entries.hasMoreElements()) {
                ZipEntry entry = entries.nextElement();
                String name = entry.getName();
                if (!name.startsWith(BLOCK_TAG_DIR) || !name.endsWith(".json")) continue;
                String id = "minecraft:" + name.substring(BLOCK_TAG_DIR.length(), name.length() - 5);
                try (var reader = new InputStreamReader(zip.getInputStream(entry), StandardCharsets.UTF_8)) {
                    List<JsonElement> values = new ArrayList<>();
                    JsonParser.parseReader(reader).getAsJsonObject().getAsJsonArray("values").forEach(values::add);
                    raw.put(id, values);
                }
            }
        }
        if (raw.isEmpty()) throw new IOException("no block tags in " + jar);

        Map<String, Set<Block>> resolved = new HashMap<>();
        for (String tag : raw.keySet()) resolveTag(tag, raw, resolved, new HashSet<>());
        Map<Block, List<TagKey<Block>>> byBlock = new HashMap<>();
        for (var tag : resolved.entrySet()) {
            TagKey<Block> key = TagKey.create(Registries.BLOCK, Identifier.parse(tag.getKey()));
            for (Block block : tag.getValue()) byBlock.computeIfAbsent(block, b -> new ArrayList<>()).add(key);
        }
        try {
            Method bind = Holder.Reference.class.getDeclaredMethod("bindTags", Collection.class);
            bind.setAccessible(true);
            for (Block block : BuiltInRegistries.BLOCK) {
                bind.invoke(block.builtInRegistryHolder(), byBlock.getOrDefault(block, List.of()));
            }
        } catch (ReflectiveOperationException e) {
            throw new IOException("bind block tags", e);
        }
        System.err.printf("bound %d block tags from %s%n", resolved.size(), jar);
    }

    private static Set<Block> resolveTag(
        String tag,
        Map<String, List<JsonElement>> raw,
        Map<String, Set<Block>> resolved,
        Set<String> visiting
    ) throws IOException {
        Set<Block> done = resolved.get(tag);
        if (done != null) return done;
        List<JsonElement> values = raw.get(tag);
        if (values == null) throw new IOException("unknown block tag #" + tag);
        if (!visiting.add(tag)) throw new IOException("block tag cycle at #" + tag);
        Set<Block> blocks = new HashSet<>();
        for (JsonElement value : values) {
            boolean required = true;
            String id;
            if (value.isJsonObject()) {
                id = value.getAsJsonObject().get("id").getAsString();
                var flag = value.getAsJsonObject().get("required");
                required = flag == null || flag.getAsBoolean();
            } else {
                id = value.getAsString();
            }
            if (id.startsWith("#")) {
                String nested = Identifier.parse(id.substring(1)).toString();
                if (!raw.containsKey(nested) && !required) continue;
                blocks.addAll(resolveTag(nested, raw, resolved, visiting));
            } else {
                var block = BuiltInRegistries.BLOCK.get(Identifier.parse(id));
                if (block.isEmpty()) {
                    if (required) throw new IOException("block tag #" + tag + " names unknown block " + id);
                    continue;
                }
                blocks.add(block.get().value());
            }
        }
        visiting.remove(tag);
        resolved.put(tag, blocks);
        return blocks;
    }
}

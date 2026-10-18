import com.google.gson.*;
import com.mojang.serialization.*;
import java.nio.file.*;
import java.util.*;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.data.registries.VanillaRegistries;
import net.minecraft.core.*;
import net.minecraft.core.registries.Registries;
import net.minecraft.resources.*;
import net.minecraft.world.level.LevelHeightAccessor;
import net.minecraft.world.level.biome.*;
import net.minecraft.world.level.levelgen.*;
import net.minecraft.world.level.levelgen.synth.NormalNoise;

/** Local, headless 26.1.2 oracle. Outputs are Mojang data and must remain ignored. */
public final class VanillaWorldgenOracle {
    private static final Gson GSON = new GsonBuilder().setPrettyPrinting().create();
    private static <T> void export(HolderLookup.Provider lookup, ResourceKey<? extends Registry<T>> key,
                                  Codec<T> codec, Path root) throws Exception {
        var ops = lookup.createSerializationContext(JsonOps.INSTANCE);
        for (var entry : lookup.lookupOrThrow(key).listElements().toList()) {
            var id = entry.key().identifier();
            Path path = root.resolve(id.getNamespace()).resolve(key.identifier().getPath()).resolve(id.getPath() + ".json");
            Files.createDirectories(path.getParent());
            Files.writeString(path, GSON.toJson(codec.encodeStart(ops, entry.value()).getOrThrow()) + "\n");
        }
    }
    @SuppressWarnings({"rawtypes", "unchecked"})
    private static Registry<?> materialize(HolderLookup.RegistryLookup<?> lookup) {
        MappedRegistry registry = new MappedRegistry(lookup.key(), Lifecycle.stable());
        lookup.listElements().forEach(entry -> registry.register(entry.key(), entry.value(), RegistrationInfo.BUILT_IN));
        return registry.freeze();
    }
    private static void biomeTag(Path root, String tag, Set<String> values) throws Exception {
        String[] parts = tag.split(":", 2);
        var file = root.resolve(parts[0]).resolve("tags/worldgen/biome").resolve(parts[1] + ".json");
        for (var value : JsonParser.parseString(Files.readString(file)).getAsJsonObject().getAsJsonArray("values")) {
            String id = value.isJsonPrimitive() ? value.getAsString() : value.getAsJsonObject().get("id").getAsString();
            if (id.startsWith("#")) biomeTag(root, id.substring(1), values);
            else values.add(id);
        }
    }
    private static void bindStructureBiomes(HolderLookup.Provider lookup, Path data) throws Exception {
        Map<String, Set<net.minecraft.tags.TagKey<Biome>>> membership = new HashMap<>();
        Path tagRoot = data.resolve("minecraft/tags/worldgen/biome");
        try (var files = Files.walk(tagRoot)) {
            for (var file : files.filter(p -> p.toString().endsWith(".json")).toList()) {
                String name = "minecraft:" + tagRoot.relativize(file).toString().replace('\\', '/').replaceAll("\\.json$", "");
                Set<String> ids = new LinkedHashSet<>();
                biomeTag(data, name, ids);
                var key = net.minecraft.tags.TagKey.create(Registries.BIOME, Identifier.parse(name));
                for (String id : ids) membership.computeIfAbsent(id, unused -> new HashSet<>()).add(key);
            }
        }
        var tags = Holder.Reference.class.getDeclaredMethod("bindTags", Collection.class);
        tags.setAccessible(true);
        for (var holder : lookup.lookupOrThrow(Registries.BIOME).listElements().toList())
            tags.invoke(holder, membership.getOrDefault(holder.key().identifier().toString(), Set.of()));
        var field = net.minecraft.world.level.levelgen.structure.Structure.class.getDeclaredField("settings");
        field.setAccessible(true);
        for (var holder : lookup.lookupOrThrow(Registries.STRUCTURE).listElements().toList()) {
            var structure = holder.value();
            var tag = structure.biomes().unwrapKey();
            if (tag.isEmpty()) continue;
            Set<String> ids = new LinkedHashSet<>();
            biomeTag(data, tag.get().location().toString(), ids);
            List<Holder<Biome>> biomes = new ArrayList<>();
            for (String id : ids) biomes.add(lookup.lookupOrThrow(Registries.BIOME).getOrThrow(ResourceKey.create(Registries.BIOME, Identifier.parse(id))));
            var old = (net.minecraft.world.level.levelgen.structure.Structure.StructureSettings) field.get(structure);
            field.set(structure, new net.minecraft.world.level.levelgen.structure.Structure.StructureSettings(
                    HolderSet.direct(biomes), old.spawnOverrides(), old.step(), old.terrainAdaptation()));
        }
    }
    private static JsonArray starts(HolderLookup.Provider lookup, RegistryAccess access,
                                    NoiseBasedChunkGenerator generator, MultiNoiseBiomeSource source,
                                    RandomState random, net.minecraft.world.level.levelgen.structure.templatesystem.StructureTemplateManager templates,
                                    LevelHeightAccessor height, long seed) {
        var state = generator.createState(lookup.lookupOrThrow(Registries.STRUCTURE_SET), random, seed);
        JsonArray result = new JsonArray();
        for (var setHolder : state.possibleStructureSets()) {
            var set = setHolder.value();
            String id = setHolder.unwrapKey().orElseThrow().identifier().toString();
            if (!(set.placement() instanceof net.minecraft.world.level.levelgen.structure.placement.RandomSpreadStructurePlacement spread)) continue;
            for (int gz = -2; gz <= 2; gz++) for (int gx = -2; gx <= 2; gx++) {
                var chunk = spread.getPotentialStructureChunk(seed, gx * spread.spacing(), gz * spread.spacing());
                if (!spread.isStructureChunk(state, chunk.x(), chunk.z())) continue;
                var entries = new ArrayList<>(set.structures());
                var choice = new WorldgenRandom(new LegacyRandomSource(0));
                choice.setLargeFeatureSeed(seed, chunk.x(), chunk.z());
                int total = entries.stream().mapToInt(e -> e.weight()).sum();
                while (!entries.isEmpty()) {
                    int index = 0;
                    if (entries.size() > 1) {
                        int draw = choice.nextInt(total);
                        while ((draw -= entries.get(index).weight()) >= 0) index++;
                    }
                    var entry = entries.remove(index);
                    var structure = entry.structure().value();
                    var start = structure.generate(entry.structure(), net.minecraft.world.level.Level.OVERWORLD,
                            access, generator, source, random, templates, seed, chunk, 0, height,
                            structure.biomes()::contains);
                    if (start.isValid()) {
                        JsonObject p = new JsonObject();
                        p.addProperty("set", id);
                        p.addProperty("structure", entry.structure().unwrapKey().orElseThrow().identifier().toString());
                        p.addProperty("chunk_x", chunk.x()); p.addProperty("chunk_z", chunk.z());
                        p.addProperty("pieces", start.getPieces().size());
                        result.add(p);
                        break;
                    }
                    total -= entry.weight();
                }
            }
        }
        return result;
    }
    public static void main(String[] args) throws Exception {
        SharedConstants.tryDetectVersion();
        Bootstrap.bootStrap();
        var lookup = VanillaRegistries.createLookup();
        Path data = Path.of(args[0]);
        export(lookup, Registries.NOISE_SETTINGS, NoiseGeneratorSettings.DIRECT_CODEC, data);
        export(lookup, Registries.DENSITY_FUNCTION, DensityFunction.DIRECT_CODEC, data);
        export(lookup, Registries.NOISE, NormalNoise.NoiseParameters.DIRECT_CODEC, data);
        var preset = lookup.lookupOrThrow(Registries.MULTI_NOISE_BIOME_SOURCE_PARAMETER_LIST)
                .getOrThrow(MultiNoiseBiomeSourceParameterLists.OVERWORLD);
        var source = MultiNoiseBiomeSource.createFromPreset(preset);
        JsonObject expanded = new JsonObject();
        JsonArray biomes = new JsonArray();
        for (var pair : preset.value().parameters().values()) {
            JsonObject entry = new JsonObject();
            entry.addProperty("biome", pair.getSecond().unwrapKey().orElseThrow().identifier().toString());
            entry.add("parameters", Climate.ParameterPoint.CODEC.encodeStart(JsonOps.INSTANCE, pair.getFirst()).getOrThrow());
            biomes.add(entry);
        }
        expanded.add("biomes", biomes);
        Path expandedPath = data.resolve("minecraft/worldgen/multi_noise_biome_source_parameter_list/overworld_expanded.json");
        Files.createDirectories(expandedPath.getParent());
        Files.writeString(expandedPath, GSON.toJson(expanded) + "\n");
        // Packaged import passes only the data root. Shell export may also name
        // a report, but without seeds neither path needs tags or templates.
        if (args.length < 3) {
            if (args.length == 2) {
                JsonObject output = new JsonObject();
                output.addProperty("version", SharedConstants.getCurrentVersion().id());
                output.add("seeds", new JsonArray());
                Files.writeString(Path.of(args[1]), GSON.toJson(output) + "\n");
            }
            return;
        }
        var settings = lookup.lookupOrThrow(Registries.NOISE_SETTINGS).getOrThrow(NoiseGeneratorSettings.OVERWORLD);
        var generator = new NoiseBasedChunkGenerator(source, settings);
        var height = LevelHeightAccessor.create(settings.value().noiseSettings().minY(), settings.value().noiseSettings().height());
        var registries = lookup.listRegistries().map(VanillaWorldgenOracle::materialize).toList();
        var access = new RegistryAccess.ImmutableRegistryAccess(registries);
        bindStructureBiomes(lookup, data);
        var resources = new net.minecraft.server.packs.resources.MultiPackResourceManager(
                net.minecraft.server.packs.PackType.SERVER_DATA,
                List.of(new net.minecraft.server.packs.PathPackResources(
                        new net.minecraft.server.packs.PackLocationInfo("oracle", net.minecraft.network.chat.Component.literal("oracle"),
                                net.minecraft.server.packs.repository.PackSource.BUILT_IN, Optional.empty()), data.getParent())));
        var storage = net.minecraft.world.level.storage.LevelStorageSource.createDefault(Path.of(args[1]).getParent().resolve("oracle-storage")).createAccess("templates");
        var templates = new net.minecraft.world.level.levelgen.structure.templatesystem.StructureTemplateManager(
                resources, storage, net.minecraft.util.datafix.DataFixers.getDataFixer(),
                net.minecraft.core.registries.BuiltInRegistries.BLOCK);
        JsonObject output = new JsonObject();
        output.addProperty("version", SharedConstants.getCurrentVersion().id());
        JsonArray seeds = new JsonArray();
        for (int a = 2; a < args.length; a++) {
            long seed = Long.parseLong(args[a]);
            var random = RandomState.create(lookup, NoiseGeneratorSettings.OVERWORLD, seed);
            JsonObject seedResult = new JsonObject();
            seedResult.addProperty("seed", seed);
            JsonArray points = new JsonArray();
            for (int z = -2048; z <= 2048; z += 128) {
                for (int x = -2048; x <= 2048; x += 128) {
                    JsonObject p = new JsonObject();
                    p.addProperty("x", x); p.addProperty("y", 64); p.addProperty("z", z);
                    var target = random.sampler().sample(Math.floorDiv(x, 4), 16, Math.floorDiv(z, 4));
                    p.addProperty("biome", source.getNoiseBiome(target).unwrapKey().orElseThrow().identifier().toString());
                    p.addProperty("height", generator.getBaseHeight(x, z, Heightmap.Types.OCEAN_FLOOR_WG, height, random) - 1);
                    JsonArray climate = new JsonArray();
                    for (long v : new long[]{target.temperature(), target.humidity(), target.continentalness(), target.erosion(), target.depth(), target.weirdness()}) climate.add(v);
                    p.add("climate", climate);
                    points.add(p);
                }
            }
            seedResult.add("points", points);
            seedResult.add("structures", starts(lookup, access, generator, source, random, templates, height, seed));
            seeds.add(seedResult);
        }
        output.add("seeds", seeds);
        Files.writeString(Path.of(args[1]), GSON.toJson(output) + "\n");
        storage.close();
        resources.close();
    }
}

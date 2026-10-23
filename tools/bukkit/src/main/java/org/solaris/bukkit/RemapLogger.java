package org.solaris.bukkit;

import java.nio.file.Files;
import java.nio.file.Path;
import java.util.jar.JarEntry;
import java.util.jar.JarInputStream;
import java.util.jar.JarOutputStream;
import org.objectweb.asm.ClassReader;
import org.objectweb.asm.ClassWriter;
import org.objectweb.asm.commons.ClassRemapper;
import org.objectweb.asm.commons.SimpleRemapper;

/** Resolve the supported Logger API to the bounded guest implementation offline. */
public final class RemapLogger {
    public static void main(String[] args) throws Exception {
        if (args.length != 2) {
            throw new IllegalArgumentException("usage: RemapLogger <input.jar> <linked.jar>");
        }
        var mapping = new SimpleRemapper("java/util/logging/Logger", "org/solaris/bukkit/PluginLogger");
        try (var input = new JarInputStream(Files.newInputStream(Path.of(args[0])));
             var output = new JarOutputStream(Files.newOutputStream(Path.of(args[1])))) {
            JarEntry entry;
            while ((entry = input.getNextJarEntry()) != null) {
                byte[] bytes = input.readAllBytes();
                if (entry.getName().endsWith(".class")) {
                    var writer = new ClassWriter(0);
                    new ClassReader(bytes).accept(new ClassRemapper(writer, mapping), 0);
                    bytes = writer.toByteArray();
                }
                output.putNextEntry(new JarEntry(entry.getName()));
                output.write(bytes);
                output.closeEntry();
            }
        }
    }
}

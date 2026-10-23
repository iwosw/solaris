package org.solaris.bukkit;

import java.nio.file.*;
import java.util.*;
import java.util.jar.*;
import org.objectweb.asm.*;

/** Resolve supported annotated handlers without loading or executing plugin classes. */
public final class Inspect {
    public static void main(String[] args) throws Exception {
        List<String> handlers = new ArrayList<>();
        try (JarFile jar = new JarFile(args[0])) {
            for (JarEntry entry : Collections.list(jar.entries())) {
                if (!entry.getName().endsWith(".class")) continue;
                byte[] bytes = jar.getInputStream(entry).readAllBytes();
                new ClassReader(bytes).accept(new ClassVisitor(Opcodes.ASM9) {
                    String owner;
                    int visibility;
                    public void visit(int version, int access, String name, String signature,
                                      String parent, String[] interfaces) {
                        owner = name.replace('/', '.').replace('$', '.');
                        visibility = access;
                        if (name.startsWith("org/bukkit/") || name.startsWith("org/solaris/"))
                            throw new IllegalArgumentException("plugin bundles reserved API: " + name);
                    }
                    public MethodVisitor visitMethod(int access, String name, String descriptor,
                                                     String signature, String[] exceptions) {
                        return new MethodVisitor(Opcodes.ASM9) {
                            boolean handler, ignore;
                            String priority = "NORMAL";
                            public AnnotationVisitor visitAnnotation(String desc, boolean visible) {
                                if (!desc.equals("Lorg/bukkit/event/EventHandler;")) return null;
                                handler = true;
                                return new AnnotationVisitor(Opcodes.ASM9) {
                                    public void visit(String key, Object value) {
                                        if (key.equals("ignoreCancelled")) ignore = (Boolean) value;
                                    }
                                    public void visitEnum(String key, String type, String value) {
                                        if (key.equals("priority")) priority = value;
                                    }
                                };
                            }
                            public void visitEnd() {
                                if (!handler) return;
                                Type[] parameters = Type.getArgumentTypes(descriptor);
                                if ((visibility & Opcodes.ACC_PUBLIC) == 0
                                    || (access & Opcodes.ACC_PUBLIC) == 0
                                    || (access & Opcodes.ACC_STATIC) != 0
                                    || parameters.length != 1
                                    || !Type.getReturnType(descriptor).equals(Type.VOID_TYPE))
                                    throw new IllegalArgumentException("unsupported handler signature: " + owner + "." + name);
                                String event = parameters[0].getClassName();
                                if (!event.equals("org.bukkit.event.player.PlayerJoinEvent")
                                    && !event.equals("org.bukkit.event.block.BlockBreakEvent"))
                                    throw new IllegalArgumentException("unsupported event: " + event);
                                if (!List.of("LOWEST", "LOW", "NORMAL", "HIGH", "HIGHEST", "MONITOR").contains(priority))
                                    throw new IllegalArgumentException("unsupported event priority: " + priority);
                                handlers.add(String.join("\t", owner, name, event, priority, Boolean.toString(ignore)));
                            }
                        };
                    }
                }, ClassReader.SKIP_CODE | ClassReader.SKIP_DEBUG | ClassReader.SKIP_FRAMES);
            }
        }
        Collections.sort(handlers);
        Files.write(Path.of(args[1]), handlers);
    }
}

package org.solaris.bukkit;

import java.io.File;
import java.net.URL;
import java.net.URLClassLoader;
import org.teavm.tooling.TeaVMTargetType;
import org.teavm.tooling.TeaVMTool;
import org.teavm.tooling.ConsoleTeaVMToolLog;

/** Offline bytecode compiler; the server never loads Java classes. */
public final class Compile {
    public static void main(String[] args) throws Exception {
        if (args.length < 3) {
            throw new IllegalArgumentException("usage: Compile <main-class> <output-directory> <classes-or-jar>...");
        }
        URL[] urls = new URL[args.length - 2];
        for (int i = 2; i < args.length; i++) {
            urls[i - 2] = new File(args[i]).toURI().toURL();
        }
        try (URLClassLoader loader = new URLClassLoader(urls, Compile.class.getClassLoader())) {
            TeaVMTool tool = new TeaVMTool();
            tool.setClassLoader(loader);
            tool.setMainClass(args[0]);
            tool.setTargetType(TeaVMTargetType.WEBASSEMBLY);
            tool.setTargetDirectory(new File(args[1]));
            tool.setTargetFileName("bukkit.wasm");
            tool.setMinHeapSize(4 * 1024 * 1024);
            tool.setMaxHeapSize(32 * 1024 * 1024);
            tool.setLog(new ConsoleTeaVMToolLog(true));
            tool.generate();
            org.teavm.tooling.TeaVMProblemRenderer.describeProblems(
                tool.getDependencyInfo().getCallGraph(), tool.getProblemProvider(), tool.getLog());
            if (!tool.getProblemProvider().getSevereProblems().isEmpty()) {
                throw new IllegalStateException("TeaVM rejected unsupported reachable bytecode: "
                    + tool.getProblemProvider().getSevereProblems());
            }
        }
    }
}

package org.bukkit.command;
public class Command {
    private final String name;
    protected Command(String name) { this.name = name; }
    public String getName() { return name; }
}

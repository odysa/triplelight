package dev.triplelight;

/** Raised by every failing call; {@link #kind()} classifies the failure. */
public final class TripleLightException extends RuntimeException {
    public enum Kind { SYNTAX, UNSUPPORTED, STORAGE, LOCKED, INVALID_ARGUMENT, INTERNAL, CLOSED, GRAPH_STATE }

    private final Kind kind;

    TripleLightException(Kind kind, String message) {
        super(kind.name().toLowerCase() + ": " + message);
        this.kind = kind;
    }

    public Kind kind() { return kind; }

    static Kind fromCode(int code) {
        return switch (code) {
            case 1 -> Kind.SYNTAX;
            case 2 -> Kind.UNSUPPORTED;
            case 3 -> Kind.STORAGE;
            case 4 -> Kind.LOCKED;
            case 5 -> Kind.INVALID_ARGUMENT;
            case 7 -> Kind.GRAPH_STATE;
            default -> Kind.INTERNAL;
        };
    }
}

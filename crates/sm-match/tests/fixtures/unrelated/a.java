package alpha;

import java.nio.file.Path;

public final class PathTools {
    public static Path normalize(Path input) {
        return input.toAbsolutePath().normalize();
    }
}

package com.example.i18n;

/**
 * Non-ASCII identifiers and string literals — legal Java, and a good way to
 * catch code that confuses byte offsets with character offsets. 日本語のコメント。
 */
public class Übersetzung {

    // Greek letters are valid Java identifiers.
    private static final double π = 3.141592653589793;
    private static final String GREETING = "こんにちは、世界";
    private static final String EMOJI = "🚀 ship it — ✅";
    private static final String MIXED = "naïve café é \n tab\there";
    private static final char DEGREE = '°';

    private String 名前 = "既定";

    public double flächeninhalt(double radius) {
        return π * radius * radius;
    }

    public String grüßen(String 名前) {
        this.名前 = 名前;
        return GREETING + ", " + 名前 + " " + EMOJI;
    }

    /* Ünïcödé in a block comment: αβγδε ☃ */
    public String 描述() {
        return MIXED + DEGREE;
    }
}

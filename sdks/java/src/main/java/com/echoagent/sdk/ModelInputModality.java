package com.echoagent.sdk;

import java.util.List;

/** Content modality accepted by one configured model. */
public enum ModelInputModality {
    TEXT("text"), IMAGE("image"), AUDIO("audio"), VIDEO("video");

    private final String wireName;
    ModelInputModality(String wireName) { this.wireName = wireName; }
    public String asStr() { return wireName; }
    public static List<ModelInputModality> textOnly() { return List.of(TEXT); }
    public static List<ModelInputModality> allSupported() { return List.of(TEXT, IMAGE, AUDIO, VIDEO); }
}

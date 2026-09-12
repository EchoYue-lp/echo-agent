package com.echoagent.sdk;

import java.util.HashMap;
import java.util.Locale;
import java.util.Map;
import java.util.Set;

/** Local provider/model policy resolver; it does not own provider transport. */
public final class ModelProfileResolver {
    private final Map<String, ModelProfileOverride> providerDefaults = new HashMap<>();
    private final Map<String, ModelProfileOverride> exactModels = new HashMap<>();

    public static ModelProfileResolver newResolver() { return new ModelProfileResolver(); }

    public ModelProfileResolver registerProviderDefault(String provider, ModelProfileOverride profile) {
        providerDefaults.put(normalize(provider), profile);
        return this;
    }

    public ModelProfileResolver registerExact(String provider, String model, ModelProfileOverride profile) {
        exactModels.put(selectorKey(provider, model), profile);
        return this;
    }

    public ModelProfile resolve(String provider, String model, ProviderCapabilities capabilities) {
        ModelProfile result = ModelProfile.newProfile(model, provider, capabilities);
        ModelProfileOverride providerDefault = providerDefaults.get(normalize(provider));
        if (providerDefault != null) result = apply(result, providerDefault);
        ModelProfileOverride exact = exactModels.get(selectorKey(provider, model));
        if (exact != null) result = apply(result, exact);
        return result;
    }

    private static ModelProfile apply(ModelProfile profile, ModelProfileOverride override) {
        ProviderCapabilities capabilities = profile.capabilities();
        if (override.supportsStructuredOutput() != null) {
            capabilities = new ProviderCapabilities(capabilities.streamingToolCalls(), capabilities.namedSseEvents(),
                    capabilities.reasoningContent(), capabilities.imageInput(), capabilities.systemAsTopLevel(),
                    capabilities.ndjsonStreaming(), capabilities.toolSupport(), override.supportsStructuredOutput(),
                    capabilities.requiresVersionHeader(), capabilities.supportsParallelToolCalls(),
                    capabilities.supportsToolChoiceNone(), capabilities.tokenizerName());
        }
        Set<String> tools = new java.util.HashSet<>(profile.excludedTools());
        tools.addAll(override.excludedTools());
        return new ModelProfile(profile.provider(), profile.modelName(), capabilities,
                profile.supportsReasoning(), profile.thinkingProtocol(), profile.thinkingLevels(),
                profile.supportsImages(), profile.supportsTools(), profile.maxOutputTokens(),
                profile.supportsStreaming(), override.supportsParallelToolCalls() != null
                        ? override.supportsParallelToolCalls() : profile.supportsParallelToolCalls(),
                override.supportsToolChoiceNone() != null
                        ? override.supportsToolChoiceNone() : profile.supportsToolChoiceNone(),
                override.contextWindow() != null ? override.contextWindow() : profile.contextWindow(),
                tools, override.promptSuffix() != null ? override.promptSuffix() : profile.promptSuffix(),
                profile.tokenizerName());
    }

    private static String normalize(String value) { return value.trim().toLowerCase(Locale.ROOT); }
    private static String selectorKey(String provider, String model) { return normalize(provider) + ":" + normalize(model); }
}

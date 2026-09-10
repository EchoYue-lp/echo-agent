package com.echoagent.sdk;

import com.fasterxml.jackson.databind.JsonNode;

import java.io.IOException;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.ArrayList;
import java.util.Comparator;
import java.util.HashMap;
import java.util.List;
import java.util.Map;
import java.util.Objects;
import java.util.TreeMap;

/** Reads the generated catalog; no operation/signature is guessed in Java code. */
public final class FacadeCatalog {
    public record ResolvedOperation(String operation, String family, String method, String signature) {}
    public record CatalogFamily(String family, List<String> methods, List<ResolvedOperation> operations) {
        public CatalogFamily {
            Objects.requireNonNull(family, "family");
            methods = List.copyOf(methods);
            operations = List.copyOf(operations);
        }
    }

    private static final Comparator<ResolvedOperation> OPERATION_ORDER = Comparator
            .comparing(ResolvedOperation::operation)
            .thenComparing(ResolvedOperation::family)
            .thenComparing(ResolvedOperation::method)
            .thenComparing(ResolvedOperation::signature);

    private final Map<String, String> operationDigests = new HashMap<>();
    private final Map<String, Map<String, String>> familyDigests = new HashMap<>();
    private final Map<String, ResolvedOperation> resolvedOperations = new HashMap<>();
    private final List<ResolvedOperation> operations;
    private final List<CatalogFamily> families;
    private final String contractDigest;
    private final String sourceContractDigest;

    public FacadeCatalog(Path catalogPath, Path digestPath) throws IOException {
        JsonNode catalog = JsonSupport.MAPPER.readTree(Files.readString(catalogPath));
        if (!catalog.isObject() || !catalog.path("routes").isArray()) {
            throw new IllegalArgumentException("canonical catalog routes must be an array");
        }
        List<ResolvedOperation> parsedOperations = new ArrayList<>();
        for (JsonNode route : catalog.path("routes")) {
            if (route.has("operation") && route.get("operation").isTextual()) {
                String operation = route.get("operation").textValue();
                String family = requiredText(route, "family", "source operation family");
                String method = requiredText(route, "method", "source operation method");
                String signature = firstSignature(route.path("signature_digests"), operation);
                ResolvedOperation resolved = new ResolvedOperation(operation, family, method, signature);
                addUnique(resolvedOperations, resolved);
                operationDigests.put(operation, signature);
                parsedOperations.add(resolved);
            } else if (route.has("method") && route.get("method").isTextual()
                    && route.path("operation_signatures").isArray()) {
                String family = requiredText(route, "family", "family operation family");
                String method = route.get("method").textValue();
                Map<String, String> signatures = new HashMap<>();
                for (JsonNode item : route.path("operation_signatures")) {
                    String operation = requiredText(item, "operation", "family operation");
                    String signature = firstSignature(item.path("signature_digests"), operation);
                    ResolvedOperation resolved = new ResolvedOperation(operation, family, method, signature);
                    addUnique(resolvedOperations, resolved);
                    signatures.put(operation, signature);
                    parsedOperations.add(resolved);
                }
                if (!signatures.isEmpty() && familyDigests.put(method, signatures) != null) {
                    throw new IllegalArgumentException("duplicate family method in canonical catalog: " + method);
                }
            }
        }
        parsedOperations.sort(OPERATION_ORDER);
        operations = List.copyOf(parsedOperations);
        TreeMap<String, List<ResolvedOperation>> grouped = new TreeMap<>();
        for (ResolvedOperation operation : operations) {
            grouped.computeIfAbsent(operation.family(), ignored -> new ArrayList<>())
                    .add(operation);
        }
        List<CatalogFamily> parsedFamilies = new ArrayList<>();
        JsonNode familyMetadata = catalog.path("families");
        for (JsonNode family : familyMetadata) {
            if (!family.path("family").isTextual()) continue;
            String familyName = family.path("family").textValue();
            List<String> methods = new ArrayList<>();
            for (JsonNode method : family.path("methods")) {
                if (method.isTextual()) methods.add(method.textValue());
            }
            grouped.computeIfAbsent(familyName, ignored -> new ArrayList<>());
            List<ResolvedOperation> familyOperations = grouped.get(familyName);
            methods.addAll(familyOperations.stream().map(ResolvedOperation::method)
                    .filter(method -> !methods.contains(method)).distinct().toList());
            methods.sort(String::compareTo);
            familyOperations.sort(OPERATION_ORDER);
            parsedFamilies.add(new CatalogFamily(familyName, methods, grouped.get(familyName)));
        }
        for (var entry : grouped.entrySet()) {
            if (parsedFamilies.stream().noneMatch(family -> family.family().equals(entry.getKey()))) {
                List<String> methods = entry.getValue().stream().map(ResolvedOperation::method).distinct().sorted().toList();
                parsedFamilies.add(new CatalogFamily(entry.getKey(), methods, entry.getValue()));
            }
        }
        parsedFamilies.sort(Comparator.comparing(CatalogFamily::family));
        families = List.copyOf(parsedFamilies);

        JsonNode digests = JsonSupport.MAPPER.readTree(Files.readString(digestPath));
        contractDigest = digests.path("contract_digest").asText();
        sourceContractDigest = digests.path("source_contract_digest").asText();
    }

    private static String requiredText(JsonNode object, String field, String context) {
        JsonNode value = object.path(field);
        if (!value.isTextual() || value.textValue().isBlank()) {
            throw new IllegalArgumentException(context + " must be a non-empty string");
        }
        return value.textValue();
    }

    private static String firstSignature(JsonNode digests, String operation) {
        if (!digests.isArray() || digests.isEmpty() || !digests.get(0).isTextual()
                || digests.get(0).textValue().isBlank()) {
            throw new IllegalArgumentException("operation has no canonical signature digest: " + operation);
        }
        return digests.get(0).textValue();
    }

    private static void addUnique(Map<String, ResolvedOperation> index, ResolvedOperation operation) {
        ResolvedOperation previous = index.putIfAbsent(operation.operation(), operation);
        if (previous != null) {
            throw new IllegalArgumentException(
                    "duplicate operation identity in canonical catalog: " + operation.operation());
        }
    }

    public String signature(String operation) {
        String digest = operationDigests.get(operation);
        if (digest == null) throw new IllegalArgumentException("operation absent from canonical catalog: " + operation);
        return digest;
    }

    public String familySignature(String method, String operation) {
        Map<String, String> signatures = familyDigests.get(method);
        String digest = signatures == null ? null : signatures.get(operation);
        if (digest == null) throw new IllegalArgumentException("family operation absent from canonical catalog: " + method + "/" + operation);
        return digest;
    }

    public ResolvedOperation resolve(String operation) {
        ResolvedOperation resolved = resolvedOperations.get(operation);
        if (resolved == null) {
            throw new IllegalArgumentException("operation absent from canonical catalog: " + operation);
        }
        return resolved;
    }

    /** Returns every canonical source and family operation in stable identity order. */
    public List<ResolvedOperation> operations() { return operations; }

    /** Returns operation groups in stable family/method order. */
    public List<CatalogFamily> families() { return families; }

    public String contractDigest() { return contractDigest; }
    public String sourceContractDigest() { return sourceContractDigest; }
}

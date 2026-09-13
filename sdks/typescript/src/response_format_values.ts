export type JsonSchemaSpec = Readonly<{
  name: string;
  schema: unknown;
  strict: boolean;
}>;

export type ResponseFormat =
  | Readonly<{ type: "text" }>
  | Readonly<{ type: "json_object" }>
  | Readonly<{ type: "json_schema"; jsonSchema: JsonSchemaSpec }>;

/** Response format values; JSON schema validation remains provider-owned. */
export const ResponseFormat = Object.freeze({
  text(): ResponseFormat { return Object.freeze({ type: "text" }); },
  jsonObject(): ResponseFormat { return Object.freeze({ type: "json_object" }); },
  jsonSchema(name: string, schema: unknown): ResponseFormat {
    return Object.freeze({ type: "json_schema", jsonSchema: Object.freeze({ name, schema, strict: true }) });
  },
  isJson(format: ResponseFormat): boolean { return format.type === "json_object" || format.type === "json_schema"; },
});

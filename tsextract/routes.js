"use strict";

// NestJS route recognition -> neutral RouteFact dicts (the TypeScript twin of
// `pyextract/routes.py`'s recognizer).
//
// Recognition is STATIC and derived ENTIRELY from the SOURCE's own constructs
// (AGENTS.md rule 1): a class carrying an `@Controller(...)` decorator, whose
// methods carry an HTTP-verb decorator (`@Get`/`@Post`/`@Put`/`@Patch`/`@Delete`),
// and whose parameters carry `@Param`/`@Query`/`@Body`. The ONLY decorators read
// are @nestjs/common's framework-native ROUTING decorators; nothing here ever
// reads a schema-annotation / validation-schema dialect on the class — the
// request/response/param SHAPES come from the TypeChecker over the method's typed
// signature (via types.mapType), exactly like the DTO schemas do.
//
// A static `@Controller('books')` prefix is composed with every method path. A
// dynamic controller prefix is diagnosed and the affected controller is omitted.
//
// A RouteFact has EXACTLY the keys the host `RouteFact` DTO (deny_unknown_fields)
// requires: method, path, handler, operation_id, params, request_body, responses,
// span. A ParamFact has EXACTLY name, location, required, schema, span. A
// ResponseFact has status/body plus optional body/media metadata — body = {ref_id:<id>} or null.

const ts = require("./ts");

const docs = require("./docs");
const load = require("./load");
const types = require("./types");

// Return the `{summary, description}` keys for a routed method's JSDoc block.
//
// AGENTS.md rule 0.1 category 2: the method's own JSDoc read as PLAIN PROSE, split by
// the shared positional rule in `./docs`. `getDocumentationComment` returns the LEADING
// DESCRIPTION ONLY — JSDoc tags are excluded by the compiler itself, so this file NEVER
// reads a tag of any kind and needs no list of tags to avoid.
//
// Only ROUTED methods reach here, so an internal helper's JSDoc can never leak into the
// API surface. Empty prose is OMITTED rather than emitted as null: the host DTO defaults
// the field, and an absent key plus an explicit null would be two spellings of one fact.
function _prose(loaded, member) {
  const symbol = loaded.checker.getSymbolAtLocation(member.name);
  if (!symbol) {
    return {};
  }
  const text = ts.displayPartsToString(symbol.getDocumentationComment(loaded.checker));
  const { summary, description } = docs.split(text);
  const prose = {};
  if (summary) {
    prose.summary = summary;
  }
  if (description) {
    prose.description = description;
  }
  return prose;
}

// The HTTP-verb method decorators -> neutral method names. Recognized by NAME
// (rule 1): these are @nestjs/common's framework-native routing decorators.
const _VERB_MAP = {
  Get: "GET",
  Post: "POST",
  Put: "PUT",
  Patch: "PATCH",
  Delete: "DELETE",
};

// The parameter decorators that bind a routing param/body (by NAME, rule 1).
const _PARAM_DECORATOR = "Param"; // -> location: path
const _QUERY_DECORATOR = "Query"; // -> location: query
const _BODY_DECORATOR = "Body"; // -> request_body (a TypeRef), NOT a param

function _diagnosticsWithContext(diags, defaults) {
  return {
    warn(message, file, line, options = {}) {
      diags.warn(message, file, line, { ...options, ...defaults });
    },
  };
}

// Return the simple callee name of a decorator (`@Get('/')` -> "Get"), or null
// for a non-call / non-identifier decorator.
function _decoratorName(decorator) {
  const expr = decorator.expression;
  if (!ts.isCallExpression(expr)) {
    return null;
  }
  const callee = expr.expression;
  if (ts.isIdentifier(callee)) {
    return callee.text;
  }
  return null;
}

// Return the first argument when it is a string literal (`@Get('/')` -> "/",
// `@Param('bookId')` -> "bookId"), or null for no/dynamic first argument.
function _decoratorStringArg(decorator) {
  const expr = decorator.expression;
  if (!ts.isCallExpression(expr)) {
    return null;
  }
  const arg = expr.arguments[0];
  return arg && ts.isStringLiteralLike(arg) ? arg.text : null;
}

// Return the integer argument of a decorator call (`@HttpCode(204)` -> 204), or
// null if there is no numeric-literal argument.
function _decoratorNumberArg(decorator) {
  const expr = decorator.expression;
  if (!ts.isCallExpression(expr)) {
    return null;
  }
  for (const arg of expr.arguments) {
    if (ts.isNumericLiteral(arg)) {
      const n = Number(arg.text);
      if (Number.isInteger(n)) {
        return n;
      }
    }
  }
  return null;
}

// All decorators on a node (class/method/parameter), via the TS 4.8+/5.x helper
// (Pitfall 6 — `node.decorators` is gone; use `ts.getDecorators`).
function _decorators(node) {
  if (ts.canHaveDecorators && ts.canHaveDecorators(node)) {
    return ts.getDecorators(node) || [];
  }
  return [];
}

// Whether a class is a routing controller: it carries an `@Controller(...)`
// decorator. Recognized by NAME (rule 1).
function _controllerDecorator(classDecl) {
  for (const dec of _decorators(classDecl)) {
    if (_decoratorName(dec) === "Controller") {
      return dec;
    }
  }
  return null;
}

// Convert a NestJS route path to the neutral path: `:name` -> `{name}`. A bare
// `/` stays `/`.
function _neutralPath(raw) {
  const out = [];
  let i = 0;
  while (i < raw.length) {
    const ch = raw[i];
    if (ch === ":") {
      // Read the param name (alphanumerics / underscore) and brace it.
      let j = i + 1;
      while (j < raw.length && /[A-Za-z0-9_]/.test(raw[j])) {
        j += 1;
      }
      out.push("{" + raw.slice(i + 1, j) + "}");
      i = j;
    } else {
      out.push(ch);
      i += 1;
    }
  }
  return out.join("");
}

function _joinStaticPaths(prefix, path) {
  const normalizedPrefix = String(prefix || "").trim().replace(/^\/+|\/+$/g, "");
  const normalizedPath = String(path || "").trim();
  if (!normalizedPrefix) {
    return normalizedPath.startsWith("/") ? normalizedPath : "/" + normalizedPath;
  }
  if (!normalizedPath || normalizedPath === "/") {
    return "/" + normalizedPrefix + "/";
  }
  return "/" + normalizedPrefix + "/" + normalizedPath.replace(/^\/+|\/+$/g, "");
}

// Find the HTTP-verb decorator on a method, returning {method, path} or null.
// The FIRST verb decorator wins (one neutral route per handler). A SECOND verb
// decorator is not silently dropped (WR-03, rule 3): emit a WARN naming the extra
// verb so the discarded route is surfaced, never vanished without a signal.
function _verbDecorator(loaded, methodDecl, diags) {
  let chosen = null;
  for (const dec of _decorators(methodDecl)) {
    const name = _decoratorName(dec);
    if (name && Object.prototype.hasOwnProperty.call(_VERB_MAP, name)) {
      const raw = _decoratorStringArg(dec);
      const expr = dec.expression;
      if (ts.isCallExpression(expr) && expr.arguments.length > 0 && raw === null) {
        const sf = methodDecl.getSourceFile();
        const line = sf.getLineAndCharacterOfPosition(dec.getStart(sf)).line + 1;
        diags.warn(
          "HTTP-verb decorator '@" +
            name +
            "' has a non-literal route path; route omitted (no fallback)",
          load.relFile(loaded.targetDir, sf.fileName),
          line,
          {
            code: "source.route.unresolved",
            category: "source",
            subject: methodDecl.name.getText(sf),
          }
        );
        continue;
      }
      if (chosen === null) {
        chosen = {
          method: _VERB_MAP[name],
          // A verb decorator with no string arg defaults to the group root `/`.
          path: _neutralPath(raw === null ? "/" : raw),
        };
      } else {
        const sf = methodDecl.getSourceFile();
        const line = sf.getLineAndCharacterOfPosition(dec.getStart(sf)).line + 1;
        diags.warn(
          "method carries a second HTTP-verb decorator '@" +
            name +
            "'; only the first verb is recorded, the extra route is dropped (no fallback)",
          load.relFile(loaded.targetDir, sf.fileName),
          line,
          {
            code: "source.route.unresolved",
            category: "source",
            operation: chosen.method + " " + chosen.path,
            subject: name,
          }
        );
      }
    }
  }
  return chosen;
}

// Return the `@HttpCode(n)` override status if present and VALID on a method,
// else null. This is the SINGLE override on the method-derived rule (rule 3) —
// never a try-then-fallback chain. The host `ResponseFact.status` is a `u16`, so
// a value outside the plausible HTTP range (100–599, which also excludes the
// negatives `Number.isInteger` would otherwise pass) cannot be a valid status
// (WR-05): diagnose it and return null — the deterministic method-derived status
// then applies (the always-on default rule, not a recovery fallback).
function _httpCodeOverride(loaded, methodDecl, diags, operation) {
  for (const dec of _decorators(methodDecl)) {
    if (_decoratorName(dec) === "HttpCode") {
      const n = _decoratorNumberArg(dec);
      if (n === null) {
        const sf = methodDecl.getSourceFile();
        const line = sf.getLineAndCharacterOfPosition(dec.getStart(sf)).line + 1;
        diags.warn(
          "@HttpCode override is not an integer literal; override ignored (no fallback)",
          load.relFile(loaded.targetDir, sf.fileName),
          line,
          {
            code: "response.status.unresolved",
            category: "response",
            operation: operation,
            subject: "HttpCode",
          }
        );
        return null;
      }
      if (n >= 100 && n <= 599) {
        return n;
      }
      const sf = methodDecl.getSourceFile();
      const line = sf.getLineAndCharacterOfPosition(dec.getStart(sf)).line + 1;
      diags.warn(
        "@HttpCode(" +
          n +
          ") is outside the valid HTTP status range (100-599); override ignored (no fallback)",
        load.relFile(loaded.targetDir, sf.fileName),
        line,
        {
          code: "response.status.unresolved",
          category: "response",
          operation: operation,
          subject: "HttpCode",
        }
      );
      return null;
    }
  }
  return null;
}

// Classify a parameter's routing decorator: returns {kind, name} where kind is
// "path" (@Param), "query" (@Query) or "body" (@Body), or null if the parameter
// carries no routing decorator. `name` is the decorator's string arg (the
// param name) or null (e.g. `@Body()`).
function _paramKind(paramDecl) {
  for (const dec of _decorators(paramDecl)) {
    const dname = _decoratorName(dec);
    if (dname === _PARAM_DECORATOR) {
      return { kind: "path", name: _decoratorStringArg(dec) };
    }
    if (dname === _QUERY_DECORATOR) {
      return { kind: "query", name: _decoratorStringArg(dec) };
    }
    if (dname === _BODY_DECORATOR) {
      return { kind: "body", name: null };
    }
  }
  return null;
}

// Resolve a method's return type to a schema ref id, registering referenced
// declarations for transitive collection. Inline representable response shapes
// receive a deterministic synthetic component so ResponseFact stays ref-only.
//
// The return type is mapped through the SAME `types.mapType` discriminator every
// field/param uses (WR-01, rule 3 — ONE named-vs-inline path): the method's
// `type` annotation node carries the syntactic info `mapType` reads, so a
// nullable named return (`getX(): BookOrError | null`) resolves identically to a
// nullable named field, instead of the divergent `t.aliasSymbol` read that TS
// drops on `| null`.
function _responseRef(loaded, methodDecl, diags, registry, operation) {
  if (!methodDecl.type) {
    const sf = methodDecl.getSourceFile();
    const line = sf.getLineAndCharacterOfPosition(methodDecl.name.getStart(sf)).line + 1;
    diags.warn(
      "handler has no return type annotation; response body omitted (no fallback)",
      load.relFile(loaded.targetDir, sf.fileName),
      line,
      {
        code: "response.schema.unresolved",
        category: "response",
        operation: operation,
        subject: methodDecl.name.getText(sf),
      }
    );
    return null;
  }
  const sf = methodDecl.getSourceFile();
  const file = load.relFile(loaded.targetDir, sf.fileName);
  const line = sf.getLineAndCharacterOfPosition(methodDecl.type.getStart(sf)).line + 1;

  const mapped = types.mapReturnType(
    loaded,
    methodDecl,
    _diagnosticsWithContext(diags, {
      code: "response.schema.unresolved",
      category: "response",
      operation: operation,
      subject: methodDecl.name.getText(sf),
    }),
    registry
  );
  if (mapped.schema === null) {
    return null; // unresolvable type (mapReturnType already recorded the diagnostic)
  }
  if (mapped.optional || mapped.nullable) {
    diags.warn(
      "response type is optional/nullable at the top level, which the response contract cannot represent; body omitted",
      file,
      line
    );
    return null;
  }
  if (mapped.schema.type === "named") {
    return mapped.schema.of;
  }

  if (!registry) {
    diags.warn(
      "response type requires a synthetic schema but no schema registry was supplied; body omitted",
      file,
      line
    );
    return null;
  }
  const handler = methodDecl.name ? methodDecl.name.getText(sf) : "response";
  const name =
    handler
      .split(/[^A-Za-z0-9]+/)
      .filter(Boolean)
      .map((part) => part.charAt(0).toUpperCase() + part.slice(1))
      .join("") + "Response";
  const refId = load.schemaId(loaded.targetDir, sf.fileName, name);
  const collidesWithSource = sf.statements.some(
    (statement) =>
      (ts.isClassDeclaration(statement) || ts.isTypeAliasDeclaration(statement)) &&
      statement.name &&
      statement.name.text === name
  );
  if (collidesWithSource) {
    diags.warn(
      "synthetic response schema '" +
        refId +
        "' collides with a source declaration; body omitted",
      file,
      line
    );
    return null;
  }
  const added = registry.add(refId, {
    kind: "synthetic",
    name: name,
    body: mapped.schema,
    span: load.span(sf, methodDecl, loaded.targetDir),
  });
  if (!added) {
    diags.warn(
      "synthetic response schema '" +
        refId +
        "' collides with another response; body omitted",
      file,
      line
    );
    return null;
  }
  return refId;
}

// Build the params + request_body from a method's typed signature.
//   @Param -> location path, required true
//   @Query -> location query, required = NOT (questionToken OR default initializer)
//   @Body  -> request_body TypeRef (NOT a param)
function _buildParams(loaded, methodDecl, diags, registry, operation) {
  const sf = methodDecl.getSourceFile();
  const params = [];
  let requestBody = null;

  for (const paramDecl of methodDecl.parameters) {
    const classified = _paramKind(paramDecl);
    if (classified === null) {
      continue; // a parameter with no routing decorator is not a route fact
    }

    if (classified.kind === "body") {
      // The request body's schema: map the parameter's typed declaration to a
      // named ref (registering the DTO for transitive collection). The body is a
      // TypeRef (just the id), not a full param.
      const mapped = types.mapType(
        loaded,
        paramDecl,
        _diagnosticsWithContext(diags, {
          code: "request.body.unresolved",
          category: "request_body",
          operation: operation,
          subject: paramDecl.name.getText(sf),
        }),
        registry
      );
      if (mapped.schema !== null && mapped.schema.type === "named") {
        if (requestBody === null) {
          requestBody = { ref_id: mapped.schema.of };
        } else {
          // A SECOND @Body on the same handler is ambiguous; surface it rather
          // than silently keeping the first (WR-04, rule 3).
          const line =
            sf.getLineAndCharacterOfPosition(paramDecl.getStart(sf)).line + 1;
          diags.warn(
            "handler has more than one @Body parameter; only the first is recorded, the extra body is dropped (no fallback)",
            load.relFile(loaded.targetDir, sf.fileName),
            line,
            {
              code: "request.body.unresolved",
              category: "request_body",
              operation: operation,
              subject: paramDecl.name.getText(sf),
            }
          );
        }
      } else if (mapped.schema !== null) {
        const line =
          sf.getLineAndCharacterOfPosition(paramDecl.getStart(sf)).line + 1;
        diags.warn(
          "@Body parameter is not a named DTO type; request body omitted (no fallback)",
          load.relFile(loaded.targetDir, sf.fileName),
          line,
          {
            code: "request.body.unresolved",
            category: "request_body",
            operation: operation,
            subject: paramDecl.name.getText(sf),
          }
        );
      }
      continue;
    }

    // A @Param / @Query parameter -> a ParamFact. The neutral param name is the
    // decorator's string arg (the wire name).
    const name = classified.name;
    if (name === null) {
      const line =
        sf.getLineAndCharacterOfPosition(paramDecl.getStart(sf)).line + 1;
      diags.warn(
        "@" +
          (classified.kind === "path" ? "Param" : "Query") +
          " has no name argument; param omitted (no fallback)",
        load.relFile(loaded.targetDir, sf.fileName),
        line,
        {
          code: "request.parameter.unresolved",
          category: "request_parameter",
          operation: operation,
          subject: paramDecl.name.getText(sf),
        }
      );
      continue;
    }

    const mapped = types.mapType(
      loaded,
      paramDecl,
      _diagnosticsWithContext(diags, {
        code: "request.parameter.unresolved",
        category: "request_parameter",
        operation: operation,
        subject: name,
      }),
      registry
    );
    if (mapped.schema === null) {
      continue; // rule 3: unresolvable param omitted (diagnostic already recorded)
    }

    // required: a path param is always required; a query param is required
    // unless it is optional (`?`) or carries a default initializer.
    let required;
    if (classified.kind === "path") {
      required = true;
    } else {
      const optional = !!paramDecl.questionToken || !!paramDecl.initializer;
      required = !optional;
    }

    // The param span anchors to the parameter-name line (the chosen convention).
    const nameNode = paramDecl.name;
    const pline =
      sf.getLineAndCharacterOfPosition(nameNode.getStart(sf)).line + 1;
    params.push({
      name: name,
      location: classified.kind,
      required: required,
      schema: mapped.schema,
      span: {
        file: load.relFile(loaded.targetDir, sf.fileName),
        start_line: pline,
        end_line: pline,
      },
    });
  }

  return { params, requestBody };
}

// Recognize the NestJS controller(s) in a loaded program -> a list of RouteFact
// dicts, seeding `registry` with every referenced DTO for transitive collection.
function recognizeNestController(loaded, diags, registry) {
  const routes = [];

  for (const sf of loaded.program.getSourceFiles()) {
    if (sf.isDeclarationFile) {
      continue;
    }
    if (!load.underTarget(loaded.targetDir, sf.fileName)) {
      continue; // the target's source only (one shared rule — load.underTarget)
    }
    const rel = load.relFile(loaded.targetDir, sf.fileName);

    sf.forEachChild((node) => {
      if (!ts.isClassDeclaration(node) || !node.name) {
        return;
      }
      const controller = _controllerDecorator(node);
      if (controller === null) {
        return; // not a routing controller
      }
      const controllerCall = controller.expression;
      let controllerPrefix = "";
      if (controllerCall.arguments.length > 0) {
        const prefixArg = controllerCall.arguments[0];
        if (!ts.isStringLiteralLike(prefixArg)) {
          const line =
            sf.getLineAndCharacterOfPosition(controller.getStart(sf)).line + 1;
          diags.warn(
            "@Controller prefix is dynamic; routes for this controller are omitted",
            rel,
            line
          );
          return;
        }
        controllerPrefix = prefixArg.text;
      }

      for (const member of node.members) {
        if (!ts.isMethodDeclaration(member) || !member.name) {
          continue;
        }
        const verb = _verbDecorator(loaded, member, diags);
        if (verb === null) {
          continue; // a method with no HTTP-verb decorator is not a route
        }
        const handler = member.name.getText(sf);
        const operation = verb.method + " " + verb.path;

        const { params, requestBody } = _buildParams(
          loaded,
          member,
          diags,
          registry,
          operation
        );

        const bodyRef = _responseRef(
          loaded,
          member,
          diags,
          registry,
          operation
        );

        // Status is METHOD-DERIVED (typed POST -> 201, else 200), overridden by
        // an explicit @HttpCode(n). A single deterministic rule (rule 3) — the
        // override is read first, the method-default applied otherwise; never a
        // try-typed-then-fallback chain.
        const override = _httpCodeOverride(loaded, member, diags, operation);
        const status =
          override !== null ? override : verb.method === "POST" ? 201 : 200;

        const responses = [
          {
            status: status,
            body: bodyRef === null ? null : { ref_id: bodyRef },
            content_types: ["application/json"],
          },
        ];

        // The operation span anchors to the method-name line (the chosen
        // convention, matching param=name-line / schema=decl-name-line).
        const opLine =
          sf.getLineAndCharacterOfPosition(member.name.getStart(sf)).line + 1;

        routes.push({
          method: verb.method,
          path: _joinStaticPaths(controllerPrefix, verb.path),
          handler: handler,
          operation_id: handler,
          params: params,
          request_body: requestBody,
          request_body_content_type:
            requestBody === null ? null : "application/json",
          responses: responses,
          ..._prose(loaded, member),
          span: {
            file: rel,
            start_line: opLine,
            end_line: opLine,
          },
        });
      }
    });
  }

  return routes;
}

module.exports = { recognizeNestController };

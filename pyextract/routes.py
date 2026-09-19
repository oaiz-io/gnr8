"""FastAPI route recognition -> neutral RouteFact dicts (the Python twin of routes.go).

Recognition is STATIC and derived from the SOURCE's own constructs (rule 1): an
``@<router>.<method>(...)`` decorator on a ``def``/``async def`` whose ``<router>`` is a
module-level binding to a ``FastAPI()`` / ``APIRouter(...)`` instance. Nothing here reads
FastAPI's runtime ``/openapi.json`` or any third-party schema dialect — the method, path,
params, body and response are all read off the decorator Call + the typed handler signature.

Static router/blueprint prefixes are code-derived route facts and are composed with decorator paths.
Dynamic prefixes are diagnosed and the affected binding is omitted.

A RouteFact has EXACTLY the keys the host ``RouteFact`` DTO (``deny_unknown_fields``)
requires: ``method, path, handler, operation_id, params, request_body, responses, span``.
A ParamFact has EXACTLY ``name, location, required, schema, span``. A ResponseFact has
``status, body`` plus optional body/media metadata.
"""

import ast

from pyextract import docs, symtab, types

#: The HTTP verbs an ``@<router>.<verb>(...)`` decorator may name (FastAPI route methods).
_HTTP_METHODS = frozenset(
    {"get", "post", "put", "delete", "patch", "head", "options", "trace"}
)

#: The constructor names that bind a route-registering instance (recognized by NAME, rule 1).
_ROUTER_CTORS = frozenset({"FastAPI", "APIRouter"})

#: The constructor names that bind a Flask route-registering instance (rule 1, by NAME).
_FLASK_CTORS = frozenset({"Flask", "Blueprint"})

#: HTTP methods that carry no request body — a request.json read here is never a body fact.
_BODYLESS_METHODS = frozenset({"GET", "HEAD", "DELETE"})


def _prose(stmt):
    """Return ``{"summary": ..., "description": ...}`` keys for a routed handler's docstring.

    AGENTS.md rule 0.1 category 2: the handler's own docstring read as PLAIN PROSE, split
    by the shared positional rule in :mod:`pyextract.docs`. Nothing here matches on
    docstring CONTENT — that is what keeps this a documentation convention rather than a
    dialect. Only ROUTED handlers reach here, so an internal helper's docstring can never
    leak into the API surface.

    Empty prose is OMITTED rather than emitted as ``None``: the host DTO defaults the
    field, so an absent key and an explicit null would be two spellings of one fact.
    """
    summary, description = docs.split(ast.get_docstring(stmt))
    prose = {}
    if summary:
        prose["summary"] = summary
    if description:
        prose["description"] = description
    return prose


class _ContextDiags:
    """Attach operation-aware defaults to diagnostics emitted by shared type mapping."""

    def __init__(self, diags, **defaults):
        self._diags = diags
        self._defaults = defaults

    def warn(self, message, file, line, **options):
        merged = dict(options)
        merged.update(self._defaults)
        self._diags.warn(message, file, line, **merged)


def _span(abs_path, node):
    line = getattr(node, "lineno", 0)
    return {"file": abs_path, "start_line": line, "end_line": line}


def _ctor_name(call):
    """Return the simple constructor name of a Call (``FastAPI`` / ``APIRouter``) or None."""
    if not isinstance(call, ast.Call):
        return None
    name = types._name_of(call.func)
    return name.split(".")[-1] if name else None


def _const_kwarg(call, key):
    """Return the Constant value of keyword ``key`` on a Call, else None."""
    for kw in call.keywords:
        if kw.arg == key and isinstance(kw.value, ast.Constant):
            return kw.value.value
    return None


def _static_prefix(call, key, diags, abs_path, line, label):
    """Return a declared static string prefix, or ``None`` after diagnosing a dynamic one."""
    for kw in call.keywords:
        if kw.arg != key:
            continue
        if isinstance(kw.value, ast.Constant) and isinstance(kw.value.value, str):
            return kw.value.value
        diags.warn(
            "{} has dynamic {}=; routes for this binding are omitted".format(label, key),
            abs_path,
            line,
        )
        return None
    return ""


def _join_static_paths(prefix, path):
    """Compose a static framework prefix and method path with one slash at the seam."""
    prefix = prefix.strip()
    path = path.strip()
    if not prefix:
        return path if path.startswith("/") else "/" + path
    normalized_prefix = "/" + prefix.strip("/")
    if path in ("", "/"):
        return normalized_prefix + "/"
    return normalized_prefix + "/" + path.strip("/")


def _registration_prefixes(module, method_name, keyword, bindings, diags):
    """Fold static include/register prefixes into bindings declared in the same module."""
    for stmt in module.tree.body:
        value = stmt.value if isinstance(stmt, ast.Expr) else None
        if not isinstance(value, ast.Call) or not isinstance(value.func, ast.Attribute):
            continue
        if value.func.attr != method_name or not value.args:
            continue
        binding = value.args[0]
        if not isinstance(binding, ast.Name) or binding.id not in bindings:
            continue
        prefix = _static_prefix(
            value,
            keyword,
            diags,
            module.abs_path,
            getattr(stmt, "lineno", 0),
            method_name,
        )
        if prefix is None:
            del bindings[binding.id]
            continue
        bindings[binding.id] = _join_static_paths(prefix, bindings[binding.id])


def _import_targets(module):
    """Map local imported names to ``(dotted module, exported name)``."""
    targets = {}
    package = module.dotted.split(".")[:-1]
    for stmt in module.tree.body:
        if not isinstance(stmt, ast.ImportFrom):
            continue
        base_parts = list(package)
        if stmt.level:
            keep = max(0, len(package) - (stmt.level - 1))
            base_parts = base_parts[:keep]
        else:
            base_parts = []
        if stmt.module:
            base_parts.extend(stmt.module.split("."))
        dotted = ".".join(base_parts)
        for alias in stmt.names:
            local = alias.asname or alias.name
            targets[local] = (dotted, alias.name)
    return targets


def _external_registration_prefixes(modules, method_name, keyword, diags):
    """Resolve static include/register prefixes applied to imported router bindings."""
    prefixes = {}
    for module in sorted(modules, key=lambda item: item.dotted):
        imports = _import_targets(module)
        calls = [node for node in ast.walk(module.tree) if isinstance(node, ast.Call)]
        calls.sort(key=lambda node: (getattr(node, "lineno", 0), getattr(node, "col_offset", 0)))
        for call in calls:
            if not isinstance(call.func, ast.Attribute) or call.func.attr != method_name:
                continue
            if not call.args or not isinstance(call.args[0], ast.Name):
                continue
            target = imports.get(call.args[0].id)
            if target is None:
                continue
            prefix = _static_prefix(
                call,
                keyword,
                diags,
                module.abs_path,
                getattr(call, "lineno", 0),
                method_name,
            )
            if target in prefixes:
                diags.warn(
                    "{} registers imported binding {} more than once; routes are omitted"
                    .format(method_name, call.args[0].id),
                    module.abs_path,
                    getattr(call, "lineno", 0),
                )
                prefixes[target] = None
            else:
                prefixes[target] = prefix
    return prefixes


def _router_bindings(module, diags, external_prefixes):
    """Map each module-level router/app variable name -> its composed static prefix.

    ``app = FastAPI(title=..)`` -> ``{"app": ""}``;
    ``router = APIRouter(prefix="/books")`` -> ``{"router": "/books"}``.
    """
    bindings = {}
    for stmt in module.tree.body:
        if not isinstance(stmt, ast.Assign):
            continue
        if len(stmt.targets) != 1 or not isinstance(stmt.targets[0], ast.Name):
            continue
        ctor = _ctor_name(stmt.value)
        if ctor in _ROUTER_CTORS:
            prefix = _static_prefix(
                stmt.value,
                "prefix",
                diags,
                module.abs_path,
                getattr(stmt, "lineno", 0),
                ctor,
            )
            if prefix is not None:
                bindings[stmt.targets[0].id] = prefix
    _registration_prefixes(module, "include_router", "prefix", bindings, diags)
    for name in list(bindings):
        key = (module.dotted, name)
        if key not in external_prefixes:
            continue
        external = external_prefixes[key]
        if external is None:
            del bindings[name]
        else:
            bindings[name] = _join_static_paths(external, bindings[name])
    return bindings


def _route_decorator(func, bindings):
    """Return the decorator Call that registers ``func`` as a route, or None.

    Recognizes ``@<name>.<verb>(...)`` where ``<name>`` is a known router binding and
    ``<verb>`` is an HTTP method. Returns the ``ast.Call`` decorator node.
    """
    for dec in func.decorator_list:
        if not isinstance(dec, ast.Call):
            continue
        attr = dec.func
        if not isinstance(attr, ast.Attribute):
            continue
        if not isinstance(attr.value, ast.Name):
            continue
        if attr.value.id in bindings and attr.attr in _HTTP_METHODS:
            return dec
    return None


def _route_path(call):
    """Return the route path: the first positional Constant arg, VERBATIM, else None."""
    for arg in call.args:
        if isinstance(arg, ast.Constant) and isinstance(arg.value, str):
            return arg.value
    return None


def _path_param_names(path):
    """Return the set of ``{name}`` template names embedded in a route path."""
    names = set()
    i = 0
    while i < len(path):
        if path[i] == "{":
            end = path.find("}", i)
            if end == -1:
                break
            names.add(path[i + 1 : end])
            i = end + 1
        else:
            i += 1
    return names


def _positional_args(args):
    """The full positional arg list in source order: posonlyargs THEN args.

    ``args.defaults`` aligns to the END of this combined list, so positional-only
    params (``def f(a, b, /, ...)``) must be counted here or the default-alignment
    math is off-by-len(posonlyargs) (WR-06).
    """
    return list(getattr(args, "posonlyargs", [])) + list(args.args)


def _has_default(args, index):
    """Whether the positional arg at ``index`` (into ``_positional_args``) has a default.

    Python aligns defaults to the END of the combined positional list
    (posonlyargs + args): N positional args with D defaults give the last D their
    defaults.
    """
    total = len(_positional_args(args))
    num_defaults = len(args.defaults)
    return index >= total - num_defaults


def _warn_untyped_param(arg, path, method, abs_path, diags):
    """Record a handler parameter whose role/type cannot be derived statically."""
    diags.warn(
        "untyped handler parameter '{}' on {} {} is omitted; parameter location "
        "and schema cannot be inferred (no fallback)".format(arg.arg, method, path),
        abs_path,
        getattr(arg, "lineno", 0),
        code="request.parameter.unresolved",
        category="request_parameter",
        operation="{} {}".format(method, path),
        subject=arg.arg,
    )


def _positional_default(args, index):
    """Return the default AST node for a positional argument, or ``None``."""
    total = len(_positional_args(args))
    first_default = total - len(args.defaults)
    if index < first_default:
        return None
    return args.defaults[index - first_default]


def _is_depends_call(node):
    if not isinstance(node, ast.Call):
        return False
    name = types._name_of(node.func)
    return bool(name) and name.split(".")[-1] == "Depends"


def _is_dependency_parameter(annotation, default):
    """Recognize FastAPI's native ``Depends`` and ``Annotated[..., Depends()]`` forms."""
    if _is_depends_call(default):
        return True
    if not isinstance(annotation, ast.Subscript):
        return False
    base = types._subscript_value(annotation)
    if not base or base.split(".")[-1] != "Annotated":
        return False
    args = types._subscript_args(annotation)
    return any(_is_depends_call(metadata) for metadata in args[1:])


def _is_union_alias(node):
    """Whether an alias value node is a ``Union[...]`` / ``A | B`` (a standalone schema)."""
    if isinstance(node, ast.Subscript):
        base = types._subscript_value(node)
        return bool(base) and base.split(".")[-1] == "Union"
    return isinstance(node, ast.BinOp) and isinstance(node.op, ast.BitOr)


def _resolves_to_class(annotation, in_module, table):
    """Whether ``annotation`` is a bare Name/Attribute resolving to a class (a body candidate)."""
    if not isinstance(annotation, (ast.Name, ast.Attribute)):
        return None
    name = types._name_of(annotation)
    if not name:
        return None
    simple = name.split(".")[-1]
    res = table.resolve(simple, in_module)
    if res is symtab.UNRESOLVABLE:
        return None
    if res.kind == "class":
        return res.qualified_id
    return None


def _resolves_to_schema_ref(annotation, in_module, table):
    """Resolve a Name/Attribute to a standalone-schema $ref id (a class OR a Union alias).

    A model/``@dataclass`` class is a schema; a top-level ``Union`` alias (``BookOrError``)
    is ALSO emitted as a standalone schema (schemas.py) and is therefore a valid response
    body ref. A ``Literal`` alias is inline-only and never a ref (rule 3 — one source).
    """
    if not isinstance(annotation, (ast.Name, ast.Attribute)):
        return None
    name = types._name_of(annotation)
    if not name:
        return None
    simple = name.split(".")[-1]
    res = table.resolve(simple, in_module)
    if res is symtab.UNRESOLVABLE:
        return None
    if res.kind == "class":
        return res.qualified_id
    if res.kind == "alias" and _is_union_alias(res.node):
        return res.qualified_id
    return None


def _build_params(func, path, method, in_module, abs_path, table, diags):
    """Build the param + request_body facts from the typed handler signature.

    A param whose name appears in the path template -> ``location: path`` (required);
    otherwise -> ``location: query`` (required = the param has NO default). The first
    param whose annotation resolves to a class becomes the request body ($ref) — but
    only on a body-bearing method: a model-typed param on a GET/HEAD/DELETE is not a
    request body and is omitted (no guess), matching the Flask path's ``allows_body``.
    """
    allows_body = method not in _BODYLESS_METHODS
    params = []
    request_body = None
    path_names = _path_param_names(path)

    # Positional + positional-only params: defaults END-align over the combined list.
    pos = _positional_args(func.args)
    for index, arg in enumerate(pos):
        annotation = arg.annotation
        if annotation is None:
            _warn_untyped_param(arg, path, method, abs_path, diags)
            continue
        if _is_dependency_parameter(
            annotation, _positional_default(func.args, index)
        ):
            continue
        # A parameter typed by a model/@dataclass class is the request body, not a param.
        body_ref = _resolves_to_class(annotation, in_module, table)
        if body_ref is not None and arg.arg not in path_names:
            if allows_body and request_body is None:
                request_body = {"ref_id": body_ref}
            elif allows_body:
                diags.warn(
                    "handler has more than one typed request body; only the first "
                    "is recorded (no fallback)",
                    abs_path,
                    getattr(arg, "lineno", 0),
                    code="request.body.unresolved",
                    category="request_body",
                    operation="{} {}".format(method, path),
                    subject=arg.arg,
                )
            else:
                diags.warn(
                    "model-typed parameter '{}' on bodyless operation {} {} is "
                    "omitted (no fallback)".format(arg.arg, method, path),
                    abs_path,
                    getattr(arg, "lineno", 0),
                    code="request.body.unresolved",
                    category="request_body",
                    operation="{} {}".format(method, path),
                    subject=arg.arg,
                )
            continue

        in_path = arg.arg in path_names
        schema, _nullable = types.map_field_annotation(
            annotation,
            in_module,
            table,
            _ContextDiags(
                diags,
                code="request.parameter.unresolved",
                category="request_parameter",
                operation="{} {}".format(method, path),
                subject=arg.arg,
            ),
        )
        if schema is None:
            continue
        if in_path:
            required = True
        else:
            required = not _has_default(func.args, index)
        params.append(
            {
                "name": arg.arg,
                "location": "path" if in_path else "query",
                "required": required,
                "schema": schema,
                "span": _span(abs_path, arg),
            }
        )

    # Keyword-only params (after ``*``): FastAPI commonly uses these for query
    # params. They are NOT in ``args.args``, so without this loop they would be
    # silently dropped (WR-06). Required-ness is per-slot: ``kw_defaults[i] is
    # None`` means no default (required). A kwonly param is never a path param.
    kwonly = func.args.kwonlyargs
    kw_defaults = func.args.kw_defaults
    for index, arg in enumerate(kwonly):
        annotation = arg.annotation
        if annotation is None:
            _warn_untyped_param(arg, path, method, abs_path, diags)
            continue
        default = kw_defaults[index] if index < len(kw_defaults) else None
        if _is_dependency_parameter(annotation, default):
            continue
        body_ref = _resolves_to_class(annotation, in_module, table)
        if body_ref is not None and arg.arg not in path_names:
            if allows_body and request_body is None:
                request_body = {"ref_id": body_ref}
            elif allows_body:
                diags.warn(
                    "handler has more than one typed request body; only the first "
                    "is recorded (no fallback)",
                    abs_path,
                    getattr(arg, "lineno", 0),
                    code="request.body.unresolved",
                    category="request_body",
                    operation="{} {}".format(method, path),
                    subject=arg.arg,
                )
            else:
                diags.warn(
                    "model-typed parameter '{}' on bodyless operation {} {} is "
                    "omitted (no fallback)".format(arg.arg, method, path),
                    abs_path,
                    getattr(arg, "lineno", 0),
                    code="request.body.unresolved",
                    category="request_body",
                    operation="{} {}".format(method, path),
                    subject=arg.arg,
                )
            continue
        in_path = arg.arg in path_names
        schema, _nullable = types.map_field_annotation(
            annotation,
            in_module,
            table,
            _ContextDiags(
                diags,
                code="request.parameter.unresolved",
                category="request_parameter",
                operation="{} {}".format(method, path),
                subject=arg.arg,
            ),
        )
        if schema is None:
            continue
        if in_path:
            required = True
        else:
            has_default = index < len(kw_defaults) and kw_defaults[index] is not None
            required = not has_default
        params.append(
            {
                "name": arg.arg,
                "location": "path" if in_path else "query",
                "required": required,
                "schema": schema,
                "span": _span(abs_path, arg),
            }
        )
    return params, request_body


def _build_response(call, operation, abs_path, diags):
    """Build the response status from ``status_code=``.

    ``status_code=`` Constant -> status (default 200 when absent). Response schema selection is
    handled separately from an explicit ``response_model=`` or the handler return annotation.
    """
    keyword = next((kw for kw in call.keywords if kw.arg == "status_code"), None)
    if keyword is None:
        return 200
    status = keyword.value.value if isinstance(keyword.value, ast.Constant) else None
    if isinstance(status, int) and not isinstance(status, bool) and 100 <= status <= 599:
        return status
    diags.warn(
        "FastAPI status_code on {} is not a constant HTTP status in 100..599; "
        "using the framework default 200 (no fallback)".format(operation),
        abs_path,
        getattr(keyword.value, "lineno", 0),
        code="response.status.unresolved",
        category="response",
        operation=operation,
        subject="status_code",
    )
    return 200


def _response_annotation(call, func):
    """Select the explicit response model, otherwise the handler return annotation."""
    for kw in call.keywords:
        if kw.arg == "response_model":
            if isinstance(kw.value, ast.Constant) and kw.value.value is None:
                return None
            return kw.value
    return func.returns


def _pascal_case(name):
    return "".join(part[:1].upper() + part[1:] for part in name.split("_") if part)


def _response_schema_ref(
    annotation,
    func,
    in_module,
    abs_path,
    table,
    diags,
    synthetic_schemas,
):
    if annotation is None:
        return None
    direct = _resolves_to_schema_ref(annotation, in_module, table)
    if direct is not None:
        return direct
    mapped = types.map_annotation(annotation, in_module, table, diags)
    if mapped is None:
        return None
    if mapped.get("type") == "named":
        return mapped.get("of")

    name = _pascal_case(func.name) + "Response"
    ref_id = "{}.{}".format(in_module, name)
    if any(schema["id"] == ref_id for schema in synthetic_schemas):
        diags.warn(
            "synthetic response schema '{}' collides with another schema; response omitted"
            .format(ref_id),
            abs_path,
            getattr(func, "lineno", 0),
        )
        return None
    synthetic_schemas.append(
        {
            "id": ref_id,
            "name": name,
            "body": mapped,
            "span": _span(abs_path, func),
        }
    )
    return ref_id


def recognize_fastapi(modules, table, diags, synthetic_schemas=None):
    """Recognize FastAPI routes across ``modules`` -> a list of RouteFact dicts.

    For each module: index its router/app bindings (with their separately-recorded
    prefix), then walk every ``def``/``async def`` carrying an ``@<router>.<verb>(...)``
    decorator and assemble its method/path/params/body/response facts. Typed return annotations are
    used when ``response_model=`` is absent, collection responses receive a deterministic synthetic
    schema, and framework dependency parameters are omitted from the HTTP contract. The host
    re-sorts the routes; the sidecar stays internally deterministic.
    """
    routes = []
    if synthetic_schemas is None:
        synthetic_schemas = []
    external_prefixes = _external_registration_prefixes(
        modules, "include_router", "prefix", diags
    )
    for module in sorted(modules, key=lambda m: m.dotted):
        bindings = _router_bindings(module, diags, external_prefixes)
        if not bindings:
            continue
        abs_path = module.abs_path
        for stmt in module.tree.body:
            if not isinstance(stmt, (ast.FunctionDef, ast.AsyncFunctionDef)):
                continue
            decorator = _route_decorator(stmt, bindings)
            if decorator is None:
                continue
            route_path = _route_path(decorator)
            if route_path is None:
                diags.warn(
                    "FastAPI route decorator has no constant path; route omitted "
                    "(no fallback)",
                    abs_path,
                    getattr(stmt, "lineno", 0),
                    code="source.route.unresolved",
                    category="source",
                    subject=stmt.name,
                )
                continue
            binding_name = decorator.func.value.id
            path = _join_static_paths(bindings[binding_name], route_path)
            method = decorator.func.attr.upper()
            operation = "{} {}".format(method, path)
            params, request_body = _build_params(
                stmt, path, method, module.dotted, abs_path, table, diags
            )
            status = _build_response(decorator, operation, abs_path, diags)
            response_annotation = _response_annotation(decorator, stmt)
            response_model = next(
                (kw for kw in decorator.keywords if kw.arg == "response_model"),
                None,
            )
            intentional_empty = (
                response_model is not None
                and isinstance(response_model.value, ast.Constant)
                and response_model.value.value is None
            ) or (
                response_model is None
                and isinstance(stmt.returns, ast.Constant)
                and stmt.returns.value is None
            )
            body_ref = (
                None
                if intentional_empty
                else _response_schema_ref(
                    response_annotation,
                    stmt,
                    module.dotted,
                    abs_path,
                    table,
                    diags,
                    synthetic_schemas,
                )
            )
            if body_ref is None and not intentional_empty:
                diags.warn(
                    "FastAPI response schema on {} cannot be resolved from "
                    "response_model or the return annotation; response body omitted "
                    "(no fallback)".format(operation),
                    abs_path,
                    getattr(response_annotation or stmt, "lineno", 0),
                    code="response.schema.unresolved",
                    category="response",
                    operation=operation,
                    subject=stmt.name,
                )
            response = {
                "status": status,
                "body": {"ref_id": body_ref} if body_ref is not None else None,
                "content_types": ["application/json"],
            }
            routes.append(
                {
                    "method": method,
                    "path": path,
                    "handler": stmt.name,
                    "operation_id": stmt.name,
                    "params": params,
                    "request_body": request_body,
                    "request_body_content_type": (
                        "application/json" if request_body is not None else None
                    ),
                    "responses": [response],
                    "span": _span(abs_path, stmt),
                    **_prose(stmt),
                }
            )
    return routes


# ---------------------------------------------------------------------------
# Flask route recognition (the HONEST typed-envelope, Plan 04).
#
# A Flask route is a ``@<bp>.route("/path", methods=[...])`` decorator on a def
# whose ``<bp>`` is a module-level ``Flask()`` / ``Blueprint()`` binding. Flask is
# genuinely less typed than FastAPI: the request/response bodies are ordinary
# ``request.json`` reads unless the author OPTS IN to a typed DTO. So the Flask
# recognizer derives every fact from the SOURCE's own typed constructs (rule 1) and
# emits a DIAGNOSTIC + OMITS the fact for every untyped surface (rule 3, no fallback):
#   * a typed return annotation -> the response body $ref; status is METHOD-DERIVED
#     (POST -> 201, else 200) — a code fact, NEVER read from the docstring.
#   * a local annotated ``order: OrderInput = ...`` -> the request body $ref.
#   * a local annotated ``status: str = request.args.get(...)`` -> a typed query param.
#   * an UNTYPED ``request.json`` read / unannotated ``request.args.get(...)`` /
#     missing return annotation -> a diagnostic, no fact.
# Static Flask ``url_prefix`` values are composed into the route path.
# ---------------------------------------------------------------------------


def _flask_bindings(module, diags, external_prefixes):
    """Map each module-level Flask/Blueprint variable name -> its static ``url_prefix``.

    ``bp = Blueprint("orders", __name__, url_prefix="/orders")`` -> ``{"bp": "/orders"}``;
    ``app = Flask(__name__)`` -> ``{"app": ""}``.
    """
    bindings = {}
    for stmt in module.tree.body:
        if not isinstance(stmt, ast.Assign):
            continue
        if len(stmt.targets) != 1 or not isinstance(stmt.targets[0], ast.Name):
            continue
        ctor = _ctor_name(stmt.value)
        if ctor in _FLASK_CTORS:
            prefix = _static_prefix(
                stmt.value,
                "url_prefix",
                diags,
                module.abs_path,
                getattr(stmt, "lineno", 0),
                ctor,
            )
            if prefix is not None:
                bindings[stmt.targets[0].id] = prefix
    _registration_prefixes(module, "register_blueprint", "url_prefix", bindings, diags)
    for name in list(bindings):
        key = (module.dotted, name)
        if key not in external_prefixes:
            continue
        external = external_prefixes[key]
        if external is None:
            del bindings[name]
        else:
            bindings[name] = _join_static_paths(external, bindings[name])
    return bindings


def _flask_route_decorator(func, bindings):
    """Return the ``@<bp>.route(...)`` decorator Call registering ``func``, or None.

    Recognizes ``@<name>.route(...)`` where ``<name>`` is a known Flask/Blueprint
    binding. The HTTP method(s) live in the ``methods=[...]`` keyword (handled by the
    caller), not in the attribute name (which is always ``route``).
    """
    for dec in func.decorator_list:
        if not isinstance(dec, ast.Call):
            continue
        attr = dec.func
        if not isinstance(attr, ast.Attribute):
            continue
        if not isinstance(attr.value, ast.Name):
            continue
        if attr.value.id in bindings and attr.attr == "route":
            return dec
    return None


def _flask_methods(call):
    """Return the upper-cased HTTP methods from the ``methods=[...]`` keyword.

    ``methods=["GET", "POST"]`` -> ``["GET", "POST"]`` (source order). Absent ->
    Flask's documented default is GET; we read the SOURCE's own list and default to
    ``["GET"]`` only when the keyword is wholly absent (a code fact, not a guess).
    """
    for kw in call.keywords:
        if kw.arg == "methods" and isinstance(kw.value, (ast.List, ast.Tuple)):
            out = []
            for elt in kw.value.elts:
                if isinstance(elt, ast.Constant) and isinstance(elt.value, str):
                    out.append(elt.value.upper())
            return out
    return ["GET"]


def _flask_path(raw):
    """Convert a Flask decorator path to a neutral relative path + path-param types.

    ``"/<int:order_id>"`` -> ``("/{order_id}", {"order_id": <int64 schema>})`` (strip the
    converter, brace the name, the ``int`` converter drives the param schema). ``"/<name>"``
    (no converter) braces the name with no recorded type. A bare ``/`` stays ``/``.
    """
    out = []
    converters = {}
    i = 0
    while i < len(raw):
        ch = raw[i]
        if ch == "<":
            end = raw.find(">", i)
            if end == -1:
                out.append(ch)
                i += 1
                continue
            inner = raw[i + 1 : end]
            if ":" in inner:
                conv, name = inner.split(":", 1)
            else:
                conv, name = "", inner
            out.append("{" + name + "}")
            if conv == "int":
                converters[name] = {
                    "type": "primitive",
                    "of": {"prim": "int", "bits": 64, "signed": True},
                }
            i = end + 1
        else:
            out.append(ch)
            i += 1
    return "".join(out), converters


def _is_request_attr(node, attr):
    """Whether ``node`` is the AST for ``request.<attr>`` (e.g. ``request.json``)."""
    return (
        isinstance(node, ast.Attribute)
        and node.attr == attr
        and isinstance(node.value, ast.Name)
        and node.value.id == "request"
    )


def _is_request_args_get(node):
    """Whether ``node`` is a ``request.args.get(...)`` Call."""
    if not isinstance(node, ast.Call):
        return False
    func = node.func
    return (
        isinstance(func, ast.Attribute)
        and func.attr == "get"
        and isinstance(func.value, ast.Attribute)
        and func.value.attr == "args"
        and isinstance(func.value.value, ast.Name)
        and func.value.value.id == "request"
    )


def _flask_body_and_params(
    func, method, path, in_module, abs_path, table, diags
):
    """Walk a Flask handler body for the typed/untyped request body + query params.

    Returns ``(params, request_body)``. Each statement is inspected ONCE, top to
    bottom (deterministic, single pass — no fallback):
      * an annotated assign ``x: T = request.json``/``T(**request.json)`` whose ``T``
        resolves to a class -> the request body $ref.
      * an annotated assign ``x: str = request.args.get(...)`` -> a typed query param.
      * a PLAIN assign reading ``request.json`` -> untyped-body diagnostic, no body.
      * a PLAIN assign reading ``request.args.get(...)`` -> untyped-query diagnostic, no param.
    ``method``/``path`` name the operation in the untyped diagnostics (a code fact:
    the HTTP method and the code-derived path, never a docstring).
    """
    params = []
    request_body = None
    # A bodyless HTTP method (GET/HEAD/DELETE) has no request body by definition; a
    # request.json read in such a handler is a code smell, never an emitted body
    # fact (WR-04). The method is a code fact (the decorator's methods=[...]).
    allows_body = method not in _BODYLESS_METHODS

    for stmt in func.body:
        if isinstance(stmt, ast.AnnAssign):
            annotation = stmt.annotation
            value = stmt.value
            # Typed request body: an annotated local whose type is a class and whose
            # value reads request.json (directly or via T(**request.json)).
            body_ref = _resolves_to_class(annotation, in_module, table)
            if body_ref is not None and _reads_request_json(value):
                if allows_body and request_body is None:
                    request_body = {"ref_id": body_ref}
                elif allows_body:
                    diags.warn(
                        "handler has more than one typed request body on {} {}; only "
                        "the first is recorded (no fallback)".format(method, path),
                        abs_path,
                        getattr(stmt, "lineno", 0),
                        code="request.body.unresolved",
                        category="request_body",
                        operation="{} {}".format(method, path),
                        subject=getattr(stmt.target, "id", "request body"),
                    )
                else:
                    diags.warn(
                        "typed request body on bodyless operation {} {} is omitted "
                        "(no fallback)".format(method, path),
                        abs_path,
                        getattr(stmt, "lineno", 0),
                        code="request.body.unresolved",
                        category="request_body",
                        operation="{} {}".format(method, path),
                        subject=getattr(stmt.target, "id", "request body"),
                    )
                continue
            if _reads_request_json(value):
                diags.warn(
                    "typed request body on {} {} has an unresolvable DTO annotation; "
                    "body omitted (no fallback)".format(method, path),
                    abs_path,
                    getattr(stmt, "lineno", 0),
                    code="request.body.unresolved",
                    category="request_body",
                    operation="{} {}".format(method, path),
                    subject=getattr(stmt.target, "id", "request body"),
                )
                continue
            # Typed query param: an annotated local reading request.args.get(...).
            if _is_request_args_get(value):
                # rule 3: a query param with no usable name is not a fact we can
                # emit — an empty "name" is an invalid OpenAPI parameter. Diagnose
                # + skip rather than fabricate "".
                if not isinstance(stmt.target, ast.Name):
                    diags.warn(
                        "typed query param on {} {} has a non-name target; param "
                        "omitted (no fallback)".format(method, path),
                        abs_path,
                        getattr(stmt, "lineno", 0),
                        code="request.parameter.unresolved",
                        category="request_parameter",
                        operation="{} {}".format(method, path),
                        subject="query parameter",
                    )
                    continue
                schema, _nullable = types.map_field_annotation(
                    annotation,
                    in_module,
                    table,
                    _ContextDiags(
                        diags,
                        code="request.parameter.unresolved",
                        category="request_parameter",
                        operation="{} {}".format(method, path),
                        subject=stmt.target.id,
                    ),
                )
                if schema is not None:
                    params.append(
                        {
                            "name": stmt.target.id,
                            "location": "query",
                            "required": False,
                            "schema": schema,
                            "span": _span(abs_path, stmt),
                        }
                    )
                continue
        elif isinstance(stmt, ast.Assign):
            value = stmt.value
            target_name = (
                stmt.targets[0].id
                if len(stmt.targets) == 1 and isinstance(stmt.targets[0], ast.Name)
                else ""
            )
            # Untyped request body: a plain assign reading request.json with no DTO.
            if _reads_request_json(value):
                diags.warn(
                    "untyped request body on {} {}: read via request.json with no "
                    "typed DTO; body shape under-specified, no schema "
                    "inferred".format(method, path),
                    abs_path,
                    getattr(stmt, "lineno", 0),
                    code="request.body.unresolved",
                    category="request_body",
                    operation="{} {}".format(method, path),
                    subject=target_name or "request body",
                )
                continue
            # Untyped query param: a plain assign reading request.args.get(...).
            if _is_request_args_get(value):
                diags.warn(
                    "untyped query param '{}' on {} {}: read via request.args.get "
                    "with no annotation; param type/required-ness under-specified, "
                    "type inferred as string only".format(target_name, method, path),
                    abs_path,
                    getattr(stmt, "lineno", 0),
                    code="request.parameter.unresolved",
                    category="request_parameter",
                    operation="{} {}".format(method, path),
                    subject=target_name or "query parameter",
                )
                continue

    return params, request_body


def _reads_request_json(value):
    """Whether an assign value reads ``request.json`` (directly or ``T(**request.json)``)."""
    if value is None:
        return False
    if _is_request_attr(value, "json"):
        return True
    # T(**request.json) — a Call with a ** keyword whose value is request.json.
    if isinstance(value, ast.Call):
        for kw in value.keywords:
            if kw.arg is None and _is_request_attr(kw.value, "json"):
                return True
    return False


def recognize_flask(modules, table, diags):
    """Recognize Flask routes across ``modules`` -> a list of RouteFact dicts.

    For each module: index its Flask/Blueprint bindings (with their separately-recorded
    ``url_prefix``), then walk every ``def``/``async def`` carrying an ``@<bp>.route(...)``
    decorator and assemble ONE RouteFact PER method in its ``methods=[...]`` list. The
    status is METHOD-DERIVED for a typed handler (POST -> 201, else 200); an untyped
    handler (no resolvable return annotation) emits ``responses: []`` + a diagnostic.
    """
    routes = []
    external_prefixes = _external_registration_prefixes(
        modules, "register_blueprint", "url_prefix", diags
    )
    for module in sorted(modules, key=lambda m: m.dotted):
        bindings = _flask_bindings(module, diags, external_prefixes)
        if not bindings:
            continue
        abs_path = module.abs_path
        for stmt in module.tree.body:
            if not isinstance(stmt, (ast.FunctionDef, ast.AsyncFunctionDef)):
                continue
            decorator = _flask_route_decorator(stmt, bindings)
            if decorator is None:
                continue
            raw_path = _route_path(decorator)
            if raw_path is None:
                diags.warn(
                    "Flask route decorator has no constant path; route omitted "
                    "(no fallback)",
                    abs_path,
                    getattr(stmt, "lineno", 0),
                    code="source.route.unresolved",
                    category="source",
                    subject=stmt.name,
                )
                continue
            relative_path, path_converters = _flask_path(raw_path)
            binding_name = decorator.func.value.id
            path = _join_static_paths(bindings[binding_name], relative_path)
            methods = _flask_methods(decorator)

            # Path params (from the converters), independent of the method split.
            path_params = []
            for name in sorted(path_converters):
                path_params.append(
                    {
                        "name": name,
                        "location": "path",
                        "required": True,
                        "schema": path_converters[name],
                        "span": _span(abs_path, stmt),
                    }
                )

            # Response body: the return annotation (a typed class) drives the $ref;
            # NO annotation -> untyped-response diagnostic + responses: [] (rule 3).
            body_ref = None
            if stmt.returns is not None:
                body_ref = _resolves_to_class(stmt.returns, module.dotted, table)

            for method in methods:
                # Body/query facts are derived once per (method, handler): the body
                # walk is identical across methods, but a diagnostic must anchor to the
                # untyped node ONCE — so derive on the FIRST method only and reuse.
                query_params, request_body = _flask_body_and_params(
                    stmt,
                    method,
                    path,
                    module.dotted,
                    abs_path,
                    table,
                    diags if method == methods[0] else _NullDiags(),
                )
                if body_ref is not None:
                    response = {
                        "status": 201 if method == "POST" else 200,
                        "body": {"ref_id": body_ref},
                        "content_types": ["application/json"],
                    }
                    responses = [response]
                else:
                    if method == methods[0]:
                        reason = (
                            "handler has no return annotation"
                            if stmt.returns is None
                            else "return annotation does not resolve to a model schema"
                        )
                        diags.warn(
                            "untyped response on {} {}: {}; response shape "
                            "under-specified, no schema inferred".format(
                                method, path, reason
                            ),
                            abs_path,
                            getattr(stmt, "lineno", 0),
                            code="response.schema.unresolved",
                            category="response",
                            operation="{} {}".format(method, path),
                            subject=stmt.name,
                        )
                    responses = []
                routes.append(
                    {
                        "method": method,
                        "path": path,
                        "handler": stmt.name,
                        "operation_id": stmt.name,
                        "params": path_params + query_params,
                        "request_body": request_body,
                        "request_body_content_type": (
                            "application/json" if request_body is not None else None
                        ),
                        "responses": responses,
                        "span": _span(abs_path, stmt),
                        **_prose(stmt),
                    }
                )
    return routes


class _NullDiags:
    """A no-op diagnostics sink so a per-method re-walk never double-records a warning."""

    def warn(self, *_args, **_kwargs):
        return None

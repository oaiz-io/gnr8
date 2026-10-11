export interface ApiErrorInit {
  headers?: Headers | undefined;
  requestId?: string | undefined;
  rawBody?: string | undefined;
  jsonBody?: unknown;
  body?: unknown;
}

export class ApiError extends Error {
  public readonly headers: Headers;
  public readonly requestId?: string | undefined;
  public readonly rawBody: string;
  public readonly jsonBody: unknown;
  public readonly body: unknown;

  constructor(
    public readonly status: number,
    init: ApiErrorInit = {},
  ) {
    super(`HTTP ${status}`);
    this.name = "ApiError";
    this.headers = init.headers ?? new Headers();
    this.requestId = init.requestId;
    this.rawBody = init.rawBody ?? "";
    this.jsonBody = init.jsonBody ?? null;
    this.body = init.body ?? this.jsonBody;
  }

  isNotFound(): boolean {
    return this.status === 404;
  }
}

export class AuthConfigurationError extends Error {
  constructor(
    public readonly operationId: string,
    public readonly alternatives: readonly (readonly string[])[],
  ) {
    super(`No configured credentials satisfy operation ${operationId}`);
    this.name = "AuthConfigurationError";
  }
}

export type ResponseDecodeFailure =
  "empty_body" | "unexpected_content_type" | "invalid_json" | "invalid_text";

export interface ResponseDecodeErrorInit {
  headers?: Headers | undefined;
  requestId?: string | undefined;
  rawBody?: string | undefined;
  expectedContentType?: string | undefined;
  actualContentType?: string | undefined;
  cause?: unknown;
}

export class ResponseDecodeError extends Error {
  public readonly headers: Headers;
  public readonly requestId?: string | undefined;
  public readonly rawBody: string;
  public readonly expectedContentType: string;
  public readonly actualContentType?: string | undefined;
  public readonly cause?: unknown;

  constructor(
    public readonly failure: ResponseDecodeFailure,
    public readonly status: number,
    init: ResponseDecodeErrorInit = {},
  ) {
    super(`HTTP ${status}: response decode failed (${failure})`);
    this.name = "ResponseDecodeError";
    this.headers = init.headers ?? new Headers();
    this.requestId = init.requestId;
    this.rawBody = init.rawBody ?? "";
    this.expectedContentType = init.expectedContentType ?? "application/json";
    this.actualContentType = init.actualContentType;
    this.cause = init.cause;
  }
}

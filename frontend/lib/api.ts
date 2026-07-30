const API_BASE = process.env.NEXT_PUBLIC_API_URL || "http://localhost:8080";

export async function api<T>(
  path: string,
  options: RequestInit = {}
): Promise<T> {
  const token =
    typeof window !== "undefined" ? localStorage.getItem("token") : null;

  const headers: Record<string, string> = {
    "content-type": "application/json",
    ...(options.headers as Record<string, string>),
  };
  if (token) headers["authorization"] = `Bearer ${token}`;

  const res = await fetch(`${API_BASE}${path}`, { ...options, headers });
  if (!res.ok) {
    const text = await res.text();
    throw new Error(text || res.statusText);
  }
  if (res.status === 204) return undefined as T;
  const text = await res.text();
  if (!text) return undefined as T;
  return JSON.parse(text);
}

// Service-specific base URLs for local dev (each service on different port)
const SERVICES: Record<string, string> = {
  identity: process.env.NEXT_PUBLIC_IDENTITY_URL || "http://localhost:8080",
  catalog: process.env.NEXT_PUBLIC_CATALOG_URL || "http://localhost:8081",
  upload: process.env.NEXT_PUBLIC_UPLOAD_URL || "http://localhost:8082",
  streaming: process.env.NEXT_PUBLIC_STREAMING_URL || "http://localhost:8083",
  social: process.env.NEXT_PUBLIC_SOCIAL_URL || "http://localhost:8084",
  search: process.env.NEXT_PUBLIC_SEARCH_URL || "http://localhost:8085",
};

export function serviceUrl(service: keyof typeof SERVICES, path: string) {
  return `${SERVICES[service]}${path}`;
}

export async function svc<T>(
  service: keyof typeof SERVICES,
  path: string,
  options: RequestInit = {}
): Promise<T> {
  const token =
    typeof window !== "undefined" ? localStorage.getItem("token") : null;

  const headers: Record<string, string> = {
    "content-type": "application/json",
    ...(options.headers as Record<string, string>),
  };
  if (token) headers["authorization"] = `Bearer ${token}`;

  const res = await fetch(serviceUrl(service, path), { ...options, headers });
  if (!res.ok) {
    const text = await res.text();
    throw new Error(text || res.statusText);
  }
  if (res.status === 204) return undefined as T;
  const body = await res.text();
  if (!body) return undefined as T;
  return JSON.parse(body);
}

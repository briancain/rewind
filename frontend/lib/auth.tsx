"use client";
import { createContext, useContext, useState, useEffect, ReactNode } from "react";
import { svc } from "@/lib/api";

interface User {
  user_id: string;
  email: string;
  display_name: string;
}

interface AuthContextType {
  user: User | null;
  token: string | null;
  login: (email: string, password: string) => Promise<void>;
  register: (email: string, username: string, password: string, inviteCode: string) => Promise<void>;
  logout: () => void;
}

const AuthContext = createContext<AuthContextType | null>(null);

export function AuthProvider({ children }: { children: ReactNode }) {
  const [user, setUser] = useState<User | null>(null);
  const [token, setToken] = useState<string | null>(null);

  useEffect(() => {
    const saved = localStorage.getItem("token");
    if (saved) {
      setToken(saved);
      svc<User>("identity", "/me")
        .then((u) => setUser(u))
        .catch(() => logout());
    }
  }, []);

  async function login(email: string, password: string) {
    const res = await svc<{ token: string }>("identity", "/login", {
      method: "POST",
      body: JSON.stringify({ email, password }),
    });
    localStorage.setItem("token", res.token);
    setToken(res.token);
    const me = await svc<User>("identity", "/me");
    setUser(me);
  }

  async function register(email: string, username: string, password: string, inviteCode: string) {
    const res = await svc<{ token: string }>("identity", "/register", {
      method: "POST",
      body: JSON.stringify({ email, display_name: username, password, invite_code: inviteCode }),
    });
    localStorage.setItem("token", res.token);
    setToken(res.token);
    const me = await svc<User>("identity", "/me");
    setUser(me);
  }

  function logout() {
    // Best-effort server-side session invalidation (deletes the session row); clear local state
    // regardless of the result. Uses the token currently in localStorage.
    svc("identity", "/logout", { method: "POST" }).catch(() => {});
    localStorage.removeItem("token");
    setToken(null);
    setUser(null);
  }

  return (
    <AuthContext.Provider value={{ user, token, login, register, logout }}>
      {children}
    </AuthContext.Provider>
  );
}

export function useAuth() {
  const ctx = useContext(AuthContext);
  if (!ctx) throw new Error("useAuth must be inside AuthProvider");
  return ctx;
}

export function useRequireAuth() {
  const { user } = useAuth();
  return (action: string): boolean => {
    if (user) return true;
    alert(`Sign in to ${action}`);
    return false;
  };
}

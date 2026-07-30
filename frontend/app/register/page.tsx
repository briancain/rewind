"use client";
import { useState } from "react";
import { useAuth } from "@/lib/auth";
import { useRouter } from "next/navigation";
import Link from "next/link";

export default function RegisterPage() {
  const { register } = useAuth();
  const router = useRouter();
  const [email, setEmail] = useState("");
  const [username, setUsername] = useState("");
  const [password, setPassword] = useState("");
  const [inviteCode, setInviteCode] = useState("");
  const [error, setError] = useState("");

  async function handleSubmit(e: React.FormEvent) {
    e.preventDefault();
    try {
      await register(email, username, password, inviteCode);
      router.push("/");
    } catch (err: unknown) {
      setError(err instanceof Error ? err.message : "Registration failed");
    }
  }

  return (
    <div className="max-w-sm mx-auto mt-20">
      <h1 className="text-2xl font-bold mb-6">Register</h1>
      {error && <p className="text-red-400 mb-4">{error}</p>}
      <form onSubmit={handleSubmit} className="space-y-4">
        <input type="text" name="invite_code" id="invite_code" autoComplete="off" placeholder="Invite Code" value={inviteCode} onChange={(e) => setInviteCode(e.target.value)} className="w-full px-3 py-2 rounded bg-neutral-800 border border-neutral-700 font-mono" required />
        <input type="text" name="username" id="username" autoComplete="nickname" placeholder="Username" value={username} onChange={(e) => setUsername(e.target.value)} className="w-full px-3 py-2 rounded bg-neutral-800 border border-neutral-700" required />
        <input type="email" name="email" id="email" autoComplete="username" placeholder="Email" value={email} onChange={(e) => setEmail(e.target.value)} className="w-full px-3 py-2 rounded bg-neutral-800 border border-neutral-700" required />
        <input type="password" name="password" id="password" autoComplete="new-password" placeholder="Password" value={password} onChange={(e) => setPassword(e.target.value)} className="w-full px-3 py-2 rounded bg-neutral-800 border border-neutral-700" required />
        <button type="submit" className="w-full py-2 bg-red-600 rounded hover:bg-red-700">Register</button>
      </form>
      <p className="mt-4 text-sm text-neutral-400">Have an account? <Link href="/login" className="text-red-400">Login</Link></p>
    </div>
  );
}

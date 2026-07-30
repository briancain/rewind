"use client";
import { useState } from "react";
import { useAuth } from "@/lib/auth";
import { svc } from "@/lib/api";
import Link from "next/link";

export default function AccountPage() {
  const { user } = useAuth();
  const [current, setCurrent] = useState("");
  const [next, setNext] = useState("");
  const [confirm, setConfirm] = useState("");
  const [error, setError] = useState("");
  const [success, setSuccess] = useState(false);
  const [submitting, setSubmitting] = useState(false);

  async function handleSubmit(e: React.FormEvent) {
    e.preventDefault();
    setError("");
    setSuccess(false);

    if (next.length < 8) {
      setError("New password must be at least 8 characters.");
      return;
    }
    if (next !== confirm) {
      setError("New passwords do not match.");
      return;
    }

    setSubmitting(true);
    try {
      await svc("identity", "/change-password", {
        method: "POST",
        body: JSON.stringify({ current_password: current, new_password: next }),
      });
      setSuccess(true);
      setCurrent("");
      setNext("");
      setConfirm("");
    } catch (err: unknown) {
      setError(err instanceof Error ? err.message : "Could not change password");
    } finally {
      setSubmitting(false);
    }
  }

  if (!user) {
    return (
      <div className="max-w-sm mx-auto mt-20 text-center">
        <p className="text-neutral-400">
          Please <Link href="/login" className="text-red-400">sign in</Link> to manage your account.
        </p>
      </div>
    );
  }

  return (
    <div className="max-w-sm mx-auto mt-16">
      <h1 className="text-2xl font-bold mb-1">Account</h1>
      <p className="text-sm text-neutral-400 mb-6">{user.email}</p>

      <h2 className="text-lg font-semibold mb-3">Change password</h2>
      {error && <p className="text-red-400 mb-4 text-sm">{error}</p>}
      {success && <p className="text-green-400 mb-4 text-sm">Password changed. Other devices have been signed out.</p>}

      <form onSubmit={handleSubmit} className="space-y-4">
        {/* Hidden username field helps password managers associate the credential. */}
        <input
          type="text"
          name="username"
          autoComplete="username"
          value={user.email}
          readOnly
          hidden
        />
        <input
          type="password"
          name="current-password"
          id="current-password"
          autoComplete="current-password"
          placeholder="Current password"
          value={current}
          onChange={(e) => setCurrent(e.target.value)}
          className="w-full px-3 py-2 rounded bg-neutral-800 border border-neutral-700"
          required
        />
        <input
          type="password"
          name="new-password"
          id="new-password"
          autoComplete="new-password"
          placeholder="New password (min 8 characters)"
          value={next}
          onChange={(e) => setNext(e.target.value)}
          className="w-full px-3 py-2 rounded bg-neutral-800 border border-neutral-700"
          required
        />
        <input
          type="password"
          name="confirm-new-password"
          id="confirm-new-password"
          autoComplete="new-password"
          placeholder="Confirm new password"
          value={confirm}
          onChange={(e) => setConfirm(e.target.value)}
          className="w-full px-3 py-2 rounded bg-neutral-800 border border-neutral-700"
          required
        />
        <button
          type="submit"
          disabled={submitting}
          className="w-full py-2 bg-red-600 rounded hover:bg-red-700 disabled:opacity-50"
        >
          {submitting ? "Changing…" : "Change password"}
        </button>
      </form>
    </div>
  );
}

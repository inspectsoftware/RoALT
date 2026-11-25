import { Component, type ReactNode } from "react";

export class AppBoundary extends Component<{ children: ReactNode }, { failed: boolean }> {
  state = { failed: false };

  static getDerivedStateFromError() {
    return { failed: true };
  }

  componentDidCatch(err: unknown) {
    console.error("remote: the dashboard crashed", err);
  }

  render() {
    if (!this.state.failed) return this.props.children;
    return (
      <div className="flex min-h-screen items-center justify-center bg-[#0b0d10] px-4">
        <div className="glass-card max-w-md rounded-2xl p-6 text-center">
          <div className="text-base font-semibold text-red-300">Something went wrong</div>
          <p className="mt-1 text-sm text-zinc-400">The dashboard hit an error it can't recover from. Your paired PCs are still saved in this browser.</p>
          <button type="button" onClick={() => window.location.reload()} className="mt-4 rounded-xl bg-lime-400 px-4 py-2 text-xs font-semibold text-zinc-950">
            Reload
          </button>
        </div>
      </div>
    );
  }
}

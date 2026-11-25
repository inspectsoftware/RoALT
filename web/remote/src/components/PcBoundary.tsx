import { Component, type ReactNode } from "react";

export class PcBoundary extends Component<{ pcName: string; onForget: () => void; children: ReactNode }, { failed: boolean }> {
  state = { failed: false };

  static getDerivedStateFromError() {
    return { failed: true };
  }

  componentDidCatch(err: unknown) {
    console.error("remote: a PC view crashed", err);
  }

  render() {
    if (!this.state.failed) return this.props.children;
    return (
      <div className="glass-card mx-auto max-w-md rounded-2xl p-6 text-center">
        <div className="text-base font-semibold text-red-300">Can't show this PC</div>
        <p className="mt-1 text-sm text-zinc-400">
          <bdi>{this.props.pcName}</bdi> sent data this dashboard can't display. Update RoALT on that PC, or forget it here.
        </p>
        <div className="mt-4 flex justify-center gap-2">
          <button type="button" onClick={this.props.onForget} className="rounded-xl border border-white/10 px-4 py-2 text-xs text-zinc-300 hover:text-red-300">
            Forget this PC
          </button>
          <button type="button" onClick={() => this.setState({ failed: false })} className="rounded-xl bg-lime-400 px-4 py-2 text-xs font-semibold text-zinc-950">
            Try again
          </button>
        </div>
      </div>
    );
  }
}

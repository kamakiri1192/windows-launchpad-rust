import { StrictMode, Component, type ReactNode } from "react";
import { createRoot } from "react-dom/client";
import { HashRouter } from "react-router-dom";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { Theme } from "@radix-ui/themes";
import "@radix-ui/themes/styles.css";
import "./styles.css";
import App from "./app/App";
class ErrorBoundary extends Component<
  { children: ReactNode },
  { failed: boolean }
> {
  state = { failed: false };
  static getDerivedStateFromError() {
    return { failed: true };
  }
  render() {
    return this.state.failed ? (
      <main className="loading-state">
        <h1>画面を表示できませんでした</h1>
        <p>再読み込みしてもう一度お試しください。</p>
        <button onClick={() => location.reload()}>再読み込み</button>
      </main>
    ) : (
      this.props.children
    );
  }
}
const client = new QueryClient({
  defaultOptions: { queries: { staleTime: 30_000, retry: 1 } },
});
createRoot(document.getElementById("root")!).render(
  <StrictMode>
    <ErrorBoundary>
      <QueryClientProvider client={client}>
        <Theme
          accentColor="indigo"
          grayColor="slate"
          radius="medium"
          scaling="95%"
        >
          <HashRouter>
            <App />
          </HashRouter>
        </Theme>
      </QueryClientProvider>
    </ErrorBoundary>
  </StrictMode>,
);

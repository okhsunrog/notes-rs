import ReactMarkdown from "react-markdown";
import remarkGfm from "remark-gfm";

export default function MarkdownResponse({ children }: { children: string }) {
  return (
    <div className="assistant-markdown text-[13px] leading-relaxed">
      <ReactMarkdown remarkPlugins={[remarkGfm]}>{children}</ReactMarkdown>
    </div>
  );
}

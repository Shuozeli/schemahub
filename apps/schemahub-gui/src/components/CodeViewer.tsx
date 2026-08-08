import { Paper } from '@mantine/core';

type CodeViewerProps = {
  value: string;
  language: string;
  height?: number | string;
};

export function CodeViewer({ value, language, height = 420 }: CodeViewerProps) {
  const lines = value.split(/\r?\n/);

  return (
    <Paper withBorder radius="sm" style={{ overflow: 'hidden' }}>
      <div
        aria-label={`${language} source code`}
        className="codeViewer"
        data-language={language}
        role="region"
        style={{ height }}
        tabIndex={0}
      >
        <pre className="codeViewerCode">
          <code>
            {lines.map((line, index) => (
              <span
                className="codeViewerLine"
                data-line={index + 1}
                key={`${index}:${line}`}
              >
                {line || '\u200b'}
              </span>
            ))}
          </code>
        </pre>
      </div>
    </Paper>
  );
}

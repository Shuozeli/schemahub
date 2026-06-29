import Editor from '@monaco-editor/react';
import { Paper } from '@mantine/core';

type CodeViewerProps = {
  value: string;
  language: string;
  height?: number | string;
};

export function CodeViewer({ value, language, height = 420 }: CodeViewerProps) {
  return (
    <Paper withBorder radius="sm" style={{ overflow: 'hidden' }}>
      <Editor
        height={height}
        language={language}
        value={value}
        options={{
          readOnly: true,
          minimap: { enabled: false },
          fontSize: 13,
          lineNumbersMinChars: 3,
          scrollBeyondLastLine: false,
          wordWrap: 'off',
          renderLineHighlight: 'none',
        }}
      />
    </Paper>
  );
}


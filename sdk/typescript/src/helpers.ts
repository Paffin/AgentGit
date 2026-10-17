import { encodeBase64, utf8Encode } from './encoding.js';
import type { FileChange } from './generated/models.js';

/** A `FileChange` that puts a file. Text is UTF-8 encoded; bytes are base64-encoded as the JSON mapping requires. */
export function putFile(
  path: string,
  content: Uint8Array | string,
  options: { executable?: boolean; symlink?: boolean } = {},
): FileChange {
  const bytes = typeof content === 'string' ? utf8Encode(content) : content;
  return {
    path,
    op: 'put',
    content: encodeBase64(bytes),
    blob_id: '',
    executable: options.executable ?? false,
    file_type: options.symlink ? 'symlink' : 'regular',
  };
}

/** A `FileChange` that deletes a file. */
export function deleteFile(path: string): FileChange {
  return { path, op: 'delete', content: '', blob_id: '', executable: false, file_type: '' };
}

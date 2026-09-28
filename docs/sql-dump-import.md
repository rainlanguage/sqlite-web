# Bounded SQL dump import

Use the import session API for a streamed SQL dump. The caller sends decoded
UTF-8 text chunks one at a time; it does not need to split at SQL statement
boundaries. Keep each chunk at or below 512 KiB in UTF-8 bytes. A single SQL
statement may span chunks, up to 16 MiB. The producer's outer transaction
markers, including `BEGIN TRANSACTION;` and `COMMIT;`, are accepted. The worker
owns the actual transaction and commits only after `finishSqlDumpImport`
succeeds. An opening marker requires a matching closing marker.

```ts
const begin = await db.beginSqlDumpImport();
if (begin.error) throw begin.error;
const id = begin.value!;

try {
  const decoder = new TextDecoder();
  for await (const bytes of decompressedByteChunks) {
    // TextDecoder preserves UTF-8 characters split between byte chunks.
    for (let offset = 0; offset < bytes.length; offset += 256 * 1024) {
      const text = decoder.decode(bytes.subarray(offset, offset + 256 * 1024), { stream: true });
      const append = await db.appendSqlDumpChunk(id, text);
      if (append.error) throw append.error;
    }
  }
  const tail = decoder.decode();
  if (tail) {
    const append = await db.appendSqlDumpChunk(id, tail);
    if (append.error) throw append.error;
  }
  const finish = await db.finishSqlDumpImport(id);
  if (finish.error) throw finish.error;
} catch (error) {
  await db.cancelSqlDumpImport(id); // harmless if a failed chunk already rolled back
  throw error;
}
```

The worker serializes imports with ordinary queries and transactions. While
an import is active, those operations return an error on every client sharing
the database. A malformed statement, incomplete final statement, oversized
statement, cancellation, or worker termination rolls back the whole import.
Row-returning SQL such as `SELECT` or `INSERT ... RETURNING` is rejected because
an import does not consume query results.
An import idle for two minutes is rolled back by the worker. The caller should
retry from a clean session after any failure. Await each append before sending
the next one; concurrent import operations are rejected to keep queued SQL
bounded. If a concurrent call is rejected, await the active call and then
cancel the session before retrying.
If the leader changes before a cross-tab import response arrives, the commit
outcome is unknown. Check or reset the database before retrying that import.

The chunk limit bounds each worker message and intermediate SQL statement.
The full dump remains with the caller's stream and is never materialized as a
statement array by this API. This API does not download or decompress the dump.

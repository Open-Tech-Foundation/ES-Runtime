test("jsonl parser and streams", async () => {
  const { JSONL } = await import('runtime:serialization');
  const { JSONLDecoderStream, JSONLEncoderStream } = { JSONLDecoderStream: JSONL.DecoderStream, JSONLEncoderStream: JSONL.EncoderStream };
  const { file } = await import('runtime:fs');

  async function testStream() {
    const chunks = [
      '{"na',
      'me": "Alice", "ag',
      'e": 30}\n{"name": ',
      '"Bob"}\n',
    ];

    const readable = new ReadableStream({
      start(controller) {
        for (const chunk of chunks) {
          controller.enqueue(chunk);
        }
        controller.close();
      }
    });

    const decoder = new JSONLDecoderStream();
    const stream = readable.pipeThrough(decoder);

    let count = 0;
    for await (const value of stream) {
      count++;
    }
  }

  async function testFilePipeline() {
    const testFile = file("test_pipeline.jsonl");

    // Write first record
    const encoder1 = new JSONLEncoderStream();
    encoder1.pipeTo(testFile.writable());
    await encoder1.write({ id: 1, name: "Alice" });
    await encoder1.close();

    // Append second record
    const encoder2 = new JSONLEncoderStream();
    encoder2.pipeTo(testFile.writable({ append: true }));
    await encoder2.write({ id: 2, name: "Bob" });
    await encoder2.close();

    // Read them back
    const stream = testFile.stream().pipeThrough(new JSONLDecoderStream());
    
    let records = [];
    for await (const record of stream) {
      records.push(record);
    }

    if (records.length !== 2 || records[1].name !== "Bob") {
      throw new Error("File pipeline read mismatch!");
    }

    await testFile.delete();
  }

  async function testSkipInvalid() {
    const chunks = [
      '{"id": 1}\n',
      'INVALID JSON\n',
      '{"id": 2}\n'
    ];

    const readable = new ReadableStream({
      start(controller) {
        for (const chunk of chunks) {
          controller.enqueue(chunk);
        }
        controller.close();
      }
    });

    const decoder = new JSONLDecoderStream({ skipInvalid: true });
    const errors = [];
    decoder.onError(err => {
      errors.push(err);
    });

    const stream = readable.pipeThrough(decoder);
    const records = [];
    for await (const value of stream) {
      records.push(value);
    }

    if (records.length !== 2 || records[1].id !== 2) throw new Error("skipInvalid records mismatch!");
    if (errors.length !== 1 || errors[0].line !== 2 || errors[0].raw !== 'INVALID JSON') throw new Error("skipInvalid error mismatch!");
  }

  await testStream();
  await testFilePipeline();
  await testSkipInvalid();
});

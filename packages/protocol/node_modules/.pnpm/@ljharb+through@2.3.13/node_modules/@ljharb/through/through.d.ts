/// <reference types="node" />

import stream from 'stream';

declare function through(
    write?: (data: unknown) => void,
    end?: () => void,
    opts?: { autoDestroy?: boolean },
): through.ThroughStream;

declare namespace through {
    interface ThroughStream extends Omit<NodeJS.ReadWriteStream, 'pause' | 'resume' | 'end' | 'write'> {
        autoDestroy: boolean;
        paused: boolean;
        readable: boolean;
        writable: boolean;
        destroy: () => ThroughStream | undefined;
        end: (data: unknown) => ThroughStream | undefined;
        pause: () => ThroughStream | undefined;
        push: (chunk: unknown) => ThroughStream;
        queue: (chunk: unknown) => ThroughStream;
        resume: () => ThroughStream;
        write: (chunk: unknown) => boolean;
    }
}

export = through;
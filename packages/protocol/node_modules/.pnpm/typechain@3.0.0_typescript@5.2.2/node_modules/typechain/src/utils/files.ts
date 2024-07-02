import { parse } from 'path'

export function getFilename(path: string) {
  return parse(path).name
}

export function getFileExtension(path: string) {
  return parse(path).ext
}

import { logWarning, Manifest, ProxyDeployment } from '.';

export async function addProxyToManifest(kind: ProxyDeployment['kind'], address: string, manifest: Manifest) {
  await manifest.addProxy({ kind, address });

  if (kind !== 'transparent' && (await manifest.getAdmin())) {
    logWarning(`A proxy admin was previously deployed on this network`, [
      `This is not natively used with the current kind of proxy ('${kind}').`,
      `Changes to the admin will have no effect on this new proxy.`,
    ]);
  }
}

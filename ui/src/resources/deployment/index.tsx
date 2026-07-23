import { deploymentStateIntention } from "@/lib/color";
import { useRead } from "@/lib/hooks";
import { ICONS } from "@/lib/icons";
import { RequiredResourceComponents } from "..";
import { Types } from "komodo_client";
import { StatusBadge } from "mogh_ui";
import DeploymentTable from "./table";
import DeploymentTabs from "./tabs";
import {
  DeployDeployment,
  DestroyDeployment,
  PauseUnpauseDeployment,
  PullDeployment,
  RestartDeployment,
  StartStopDeployment,
} from "./executions";
import { useServer } from "@/resources/server";
import ResourceLink from "@/resources/link";
import { Group, Text } from "@mantine/core";
import { RunBuild } from "@/resources/build/executions";
import DockerResourceLink from "@/components/docker/link";
import ContainerPorts from "@/components/docker/container-ports";
import DeploymentUpdateAvailable from "./update-available";
import ResourceHeader from "../header";
import BatchExecutions from "@/components/batch-executions";
import NewResourceWithDeployTarget from "../new-with-deploy-target";
import { hexColorByIntention } from "mogh_ui";

export function useDeployment(id: string | undefined, useName?: boolean) {
  return useRead("ListDeployments", {}).data?.find((r) =>
    useName ? r.name === id : r.id === id,
  );
}

export function useFullDeployment(id: string) {
  return useRead(
    "GetDeployment",
    { deployment: id },
    { refetchInterval: 30_000 },
  ).data;
}

export const DeploymentComponents: RequiredResourceComponents<
  Types.DeploymentConfig,
  Types.DeploymentInfo,
  Types.DeploymentListItemInfo
> = {
  useList: () => useRead("ListDeployments", {}).data,
  useListItem: useDeployment,
  useFull: useFullDeployment,

  useResourceLinks: (deployment) => deployment?.config?.links,

  useDashboardSummaryData: () => {
    const summary = useRead(
      "GetDeploymentsSummary",
      {},
      { refetchInterval: 10_000 },
    ).data;
    const all = [
      summary?.running ?? 0,
      summary?.stopped ?? 0,
      summary?.unhealthy ?? 0,
      summary?.unknown ?? 0,
    ];
    const [running, stopped, unhealthy, unknown] = all;
    return [
      all.every((item) => item === 0) && {
        title: "Not Deployed",
        intention: "Neutral",
        value: summary?.not_deployed ?? 0,
      },
      { intention: "Good", value: running, title: "Running" },
      {
        title: "Stopped",
        intention: "Warning",
        value: stopped,
      },
      {
        title: "Unhealthy",
        intention: "Critical",
        value: unhealthy,
      },
      {
        title: "Unknown",
        intention: "Unknown",
        value: unknown,
      },
    ];
  },

  Description: () => <>Deploy individual containers.</>,

  New: (props) => <NewResourceWithDeployTarget type="Deployment" {...props} />,

  BatchExecutions: () => (
    <BatchExecutions
      type="Deployment"
      executions={[
        ["CheckDeploymentForUpdate", ICONS.UpdateAvailable],
        ["PullDeployment", ICONS.Pull],
        ["Deploy", ICONS.Deploy],
        ["RestartDeployment", ICONS.Restart],
        ["StopDeployment", ICONS.Stop],
        ["DestroyDeployment", ICONS.Destroy],
      ]}
    />
  ),

  Table: DeploymentTable,

  Icon: ({ id, size = "1rem", noColor }) => {
    const info = useRead("ListDeployments", {}).data?.find(
      (r) => r.id === id,
    )?.info;
    const color = noColor
      ? undefined
      : info &&
        hexColorByIntention(
          deploymentStateIntention(info.state, info.update_available),
        );
    return <ICONS.Deployment size={size} color={color} />;
  },

  ResourcePageHeader: ({ id }) => {
    const deployment = useDeployment(id);
    return (
      <ResourceHeader
        type="Deployment"
        id={id}
        resource={deployment}
        intent={deploymentStateIntention(
          deployment?.info.state,
          deployment?.info.update_available,
        )}
        icon={ICONS.Deployment}
        name={deployment?.name}
        state={deployment?.info.state}
        status={deployment?.info.status}
      />
    );
  },

  State: ({ id }) => {
    let info = useDeployment(id)?.info;
    return (
      <StatusBadge
        text={info?.state}
        intent={deploymentStateIntention(info?.state, info?.update_available)}
      />
    );
  },
  Info: {
    DeployTarget: ({ id }) => {
      const info = useDeployment(id)?.info;
      const server = useServer(info?.server_id);
      return server?.id ? (
        <ResourceLink type="Server" id={server?.id} />
      ) : (
        <Group gap="xs">
          <ICONS.Server size="1rem" />
          <Text>Unknown</Text>
        </Group>
      );
    },
    Image: ({ id }) => {
      const config = useFullDeployment(id)?.config;
      const info = useDeployment(id)?.info;
      return info?.build_id ? (
        <ResourceLink type="Build" id={info.build_id} />
      ) : (
        <Group gap="xs">
          <ICONS.Image size="1rem" />
          <Text>
            {info?.image.startsWith("sha256:")
              ? (
                  config?.image as Extract<
                    Types.DeploymentImage,
                    { type: "Image" }
                  >
                )?.params.image
              : info?.image.split("@")[0] || "N/A"}
          </Text>
        </Group>
      );
    },
    DockerResource: ({ id }) => {
      const deployment = useDeployment(id);
      if (
        !deployment ||
        [
          Types.DeploymentState.Unknown,
          Types.DeploymentState.NotDeployed,
        ].includes(deployment.info.state)
      ) {
        return null;
      }
      return (
        <DockerResourceLink
          type="Container"
          name={deployment.name}
          serverId={deployment.info.server_id}
        />
      );
    },
    Ports: ({ id }) => {
      const deployment = useDeployment(id);
      const container = useRead(
        "ListDockerContainers",
        {
          server: deployment?.info.server_id!,
        },
        { refetchInterval: 10_000, enabled: !!deployment?.info.server_id },
      ).data?.find((container) => container.name === deployment?.name);
      if (!container) return null;
      return (
        <ContainerPorts
          ports={container?.ports ?? []}
          serverId={deployment?.info.server_id}
        />
      );
    },
    HttpProxyUrl: ({ id }) => {
      const config = useFullDeployment(id)?.config;
      const coreInfo = useRead("GetCoreInfo", {}).data;
      const httpProxy = config?.http_proxy;
      const baseDomain = coreInfo?.ingress_base_domain;
      if (!httpProxy || !baseDomain) return null;
      const url = `https://${httpProxy.subdomain || ""}.${baseDomain}`;
      return (
        <Group gap="xs">
          <ICONS.Network size="1rem" />
          <a
            href={url}
            target="_blank"
            rel="noreferrer"
            className="text-primary hover:underline"
          >
            {url}
          </a>
        </Group>
      );
    },
    UpdateAvailable: DeploymentUpdateAvailable,
  },

  Executions: {
    RunBuild: ({ id }) => {
      const build_id = useDeployment(id)?.info.build_id;
      if (!build_id) return null;
      return <RunBuild id={build_id} />;
    },
    DeployDeployment,
    PullDeployment,
    RestartDeployment,
    PauseUnpauseDeployment,
    StartStopDeployment,
    DestroyDeployment,
  },

  Config: DeploymentTabs,

  Page: {},
};

output "dashboard_name" {
  value = aws_cloudwatch_dashboard.overview.dashboard_name
}

# Dashboards are global; the console URL just needs any region for the home endpoint.
output "dashboard_url" {
  value = "https://${var.state_region}.console.aws.amazon.com/cloudwatch/home?region=${var.state_region}#dashboards:name=${aws_cloudwatch_dashboard.overview.dashboard_name}"
}

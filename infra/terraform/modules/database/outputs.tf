output "instance_name" {
  value       = google_sql_database_instance.primary.name
  description = "Cloud SQL instance name."
}

output "connection_name" {
  value       = google_sql_database_instance.primary.connection_name
  description = "Cloud SQL connection name (project:region:instance)."
}

output "private_ip_address" {
  value       = google_sql_database_instance.primary.private_ip_address
  description = "Private IP of the Cloud SQL primary instance."
}

output "databases" {
  value       = [for db in google_sql_database.app : db.name]
  description = "Names of databases created inside the instance."
}
